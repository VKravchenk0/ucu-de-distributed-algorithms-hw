package ua.vk.ucu.dads.raft;

import org.junit.jupiter.api.Test;
import ua.vk.ucu.dads.config.NodeConfig;
import ua.vk.ucu.dads.grpc.RequestAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.ResponseAppendEntriesRPC;
import ua.vk.ucu.dads.raft.log.LogEntry;
import ua.vk.ucu.dads.raft.log.LogStore;
import ua.vk.ucu.dads.replication.ProtoMapper;
import ua.vk.ucu.dads.statemachine.Command;
import ua.vk.ucu.dads.statemachine.StateMachine;

import java.time.Duration;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

import static org.awaitility.Awaitility.await;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Exercises replication end to end through real {@link RaftNode} instances wired together by
 * {@link InProcessNetwork} (RPC handler methods called directly, no gRPC) - fast and
 * deterministic, but real election/replication logic on both sides of every exchange.
 */
class RaftNodeReplicationTest {

    private static NodeConfig.PeerInfo dummyPeer() {
        return new NodeConfig.PeerInfo("unused:0", "http://unused:0");
    }

    private static NodeConfig configFor(int nodeId, int... peerIds) {
        Map<Integer, NodeConfig.PeerInfo> peers = new HashMap<>();
        for (int peerId : peerIds) {
            peers.put(peerId, dummyPeer());
        }
        return new NodeConfig(nodeId, peers);
    }

    private static void awaitLeader(RaftNode node) {
        await().atMost(Duration.ofSeconds(5)).until(() -> node.currentState().status() == ServerStatus.LEADER);
    }

    private static RaftNode awaitSingleLeader(List<RaftNode> nodes) {
        AtomicReference<RaftNode> leader = new AtomicReference<>();
        await().atMost(Duration.ofSeconds(5)).untilAsserted(() -> {
            List<RaftNode> leaders = nodes.stream().filter(n -> n.currentState().status() == ServerStatus.LEADER).toList();
            assertEquals(1, leaders.size());
            leader.set(leaders.get(0));
        });
        return leader.get();
    }

    @Test
    void singleNodeClusterCommitsAndAppliesImmediately() throws Exception {
        InProcessNetwork network = new InProcessNetwork();
        RaftNode node = new RaftNode(configFor(1), new LogStore(), network, new StateMachine());
        network.register(1, node);
        node.start();
        try {
            awaitLeader(node);

            CommandResult result = node.submitCommand(new Command("x", Command.Action.SET, 5)).get(2, TimeUnit.SECONDS);

            assertEquals(CommandResult.Status.COMMITTED, result.status());
            assertEquals(5, result.value());
            assertEquals(1, node.currentState().commitIndex());
            assertEquals(1, node.currentState().lastApplied());
        } finally {
            node.shutdown();
        }
    }

    @Test
    void threeNodeClusterReplicatesAndConvergesOnMajority() throws Exception {
        InProcessNetwork network = new InProcessNetwork();
        List<Integer> ids = List.of(1, 2, 3);
        Map<Integer, RaftNode> nodes = new HashMap<>();
        Map<Integer, StateMachine> stateMachines = new HashMap<>();
        for (int id : ids) {
            int[] peers = ids.stream().filter(other -> other != id).mapToInt(Integer::intValue).toArray();
            StateMachine sm = new StateMachine();
            RaftNode node = new RaftNode(configFor(id, peers), new LogStore(), network, sm);
            nodes.put(id, node);
            stateMachines.put(id, sm);
            network.register(id, node);
        }
        nodes.values().forEach(RaftNode::start);
        try {
            RaftNode leader = awaitSingleLeader(nodes.values().stream().toList());

            CommandResult result = leader.submitCommand(new Command("x", Command.Action.SET, 5)).get(5, TimeUnit.SECONDS);
            assertEquals(CommandResult.Status.COMMITTED, result.status());

            await().atMost(Duration.ofSeconds(5)).untilAsserted(() ->
                    stateMachines.values().forEach(sm -> assertEquals(Map.of("x", 5), sm.snapshot())));
        } finally {
            nodes.values().forEach(RaftNode::shutdown);
        }
    }

    @Test
    void leaderBacktracksNextIndexAndOverwritesConflictingFollowerEntry() throws Exception {
        // node1 will become leader; its pre-seeded entry (term 2) is more "up to date" than
        // node2's conflicting stale entry (term 1) at the same index, so node2 both grants
        // node1's vote and, once node1 leads, initially rejects its AppendEntries (prevLogTerm
        // mismatch) before nextIndex backtracks far enough to resolve the conflict.
        LogStore leaderLog = new LogStore();
        leaderLog.append(new LogEntry(2, new Command("real", Command.Action.SET, 42)));

        LogStore followerLog = new LogStore();
        followerLog.append(new LogEntry(1, new Command("stale", Command.Action.SET, 1)));

        InProcessNetwork network = new InProcessNetwork();
        RaftNode node1 = new RaftNode(configFor(1, 2), leaderLog, network, new StateMachine());
        RaftNode node2 = new RaftNode(configFor(2, 1), followerLog, network, new StateMachine());
        network.register(1, node1);
        network.register(2, node2);
        node1.start();
        node2.start();
        try {
            awaitLeader(node1);

            await().atMost(Duration.ofSeconds(10)).untilAsserted(() -> {
                assertEquals(1, followerLog.lastIndex());
                assertEquals(2, followerLog.get(1).term());
                assertEquals("real", followerLog.get(1).command().key());
            });
        } finally {
            node1.shutdown();
            node2.shutdown();
        }
    }

    @Test
    void conflictingAppendEntriesTruncatesAndAdoptsLeaderSuffix() {
        LogStore log = new LogStore();
        log.append(new LogEntry(1, new Command("stale1", Command.Action.SET, 1))); // index 1, kept (matches)
        log.append(new LogEntry(1, new Command("stale2", Command.Action.SET, 2))); // index 2, conflicts

        RaftNode follower = new RaftNode(configFor(2, 1), log, new InProcessNetwork(), new StateMachine());

        RequestAppendEntriesRPC request = RequestAppendEntriesRPC.newBuilder()
                .setTerm(2)
                .setLeaderId(1)
                .setPrevLogIndex(1)
                .setPrevLogTerm(1)
                .addEntries(ProtoMapper.toProto(new LogEntry(2, new Command("real2", Command.Action.SET, 99))))
                .setLeaderCommit(0)
                .build();

        try {
            ResponseAppendEntriesRPC response = follower.handleAppendEntries(request);

            assertTrue(response.getSuccess());
            assertEquals(2, log.lastIndex());
            assertEquals("stale1", log.get(1).command().key());
            assertEquals(2, log.get(2).term());
            assertEquals("real2", log.get(2).command().key());
        } finally {
            follower.shutdown();
        }
    }

    @Test
    void addOnUnknownKeyFailsButAdvancesLastApplied() throws Exception {
        InProcessNetwork network = new InProcessNetwork();
        RaftNode node = new RaftNode(configFor(1), new LogStore(), network, new StateMachine());
        network.register(1, node);
        node.start();
        try {
            awaitLeader(node);

            CommandResult result = node.submitCommand(new Command("missing", Command.Action.ADD, 1)).get(2, TimeUnit.SECONDS);

            assertEquals(CommandResult.Status.FAILED, result.status());
            assertEquals(1, result.index());
            assertNotNull(result.message());
            assertEquals(1, node.currentState().lastApplied());
            assertEquals(1, node.currentState().commitIndex());
        } finally {
            node.shutdown();
        }
    }
}
