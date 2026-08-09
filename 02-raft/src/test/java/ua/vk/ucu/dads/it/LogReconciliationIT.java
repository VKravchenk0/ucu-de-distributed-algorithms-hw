package ua.vk.ucu.dads.it;

import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.testcontainers.containers.GenericContainer;

import java.time.Duration;
import java.util.List;
import java.util.Map;

import static org.awaitility.Awaitility.await;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Raft's log reconciliation (paper §5.3, Figure 7): a node that fell behind while holding
 * uncommitted entries of its own must have that divergent suffix thrown away and replaced by the
 * leader's. The leader finds where the two logs last agreed by walking {@code nextIndex} backwards
 * one step per rejected AppendEntries, then ships everything from that point on.
 *
 * <p>Rather than hand-crafting divergent logs, this test makes a real 3-node cluster produce one,
 * using {@code docker pause} as the network partition:
 *
 * <pre>
 *                       idx:  1        2         3         4
 *  1. all three nodes agree: [T:base]                            &lt;- committed everywhere
 *  2. old leader, alone:     [T:base][T:ghost1][T:ghost2]        &lt;- appended, never committed
 *  3. new leader, term T':   [T:base][T':real1][T':real2][T':real3]
 *  4. old leader rejoins ------&gt; its log must become identical to the new leader's
 * </pre>
 *
 * <p>Step 4 is the code under test. A leader initializes {@code nextIndex} optimistically to its
 * own last index + 1 for every peer, so at that point the leader believes the stale node already
 * has four entries, and has to back up three times before it finds common ground:
 *
 * <pre>
 *  prevLogIndex 4 -> rejected (past the end of the stale node's 3-entry log)
 *  prevLogIndex 3 -> rejected (term mismatch: T vs T')
 *  prevLogIndex 2 -> rejected (term mismatch: T vs T')
 *  prevLogIndex 1 -> ACCEPTED -> entries 2..4 sent, stale node truncates from index 2
 * </pre>
 *
 * <p>Note the extra leader change in step 3b. Without it this test still passes but proves much
 * less: the leader elected in step 3 was elected while its log was one entry long, so it set
 * {@code nextIndex = 2} for the absent node and - since nothing can be replicated to a paused
 * node - still holds that value when the node returns. {@code prevLogIndex = 1} then matches on
 * the first try and no backtracking happens at all. Forcing one more election after the log has
 * grown to four entries is what puts {@code nextIndex} back up at 5 and makes the walk backwards
 * mandatory. (Verified by mutation: with the {@code nextIndex--} in
 * {@code RaftNode.onAppendEntriesResponse} removed, this test fails.)
 */
class LogReconciliationIT extends RaftContainerSupport {

    private static final int NODE1_ID = 1;
    private static final int NODE2_ID = 2;
    private static final int NODE3_ID = 3;
    // Deliberately distinct from ReplicationIT's ports: this test leaves the cluster in a
    // permanently diverged state, so it gets its own containers.
    private static final Map<Integer, Integer> HTTP_PORTS = Map.of(NODE1_ID, 17010, NODE2_ID, 17011, NODE3_ID, 17012);
    private static final Map<Integer, Integer> GRPC_PORTS = Map.of(NODE1_ID, 16011, NODE2_ID, 16012, NODE3_ID, 16013);

    static GenericContainer<?> node1;
    static GenericContainer<?> node2;
    static GenericContainer<?> node3;
    static List<GenericContainer<?>> allNodes;

    @BeforeAll
    static void startCluster() {
        String gatewayIp = bridgeGatewayIp();
        node1 = newRaftNode(NODE1_ID, HTTP_PORTS.get(NODE1_ID), GRPC_PORTS.get(NODE1_ID), peersOf(gatewayIp, NODE1_ID));
        node2 = newRaftNode(NODE2_ID, HTTP_PORTS.get(NODE2_ID), GRPC_PORTS.get(NODE2_ID), peersOf(gatewayIp, NODE2_ID));
        node3 = newRaftNode(NODE3_ID, HTTP_PORTS.get(NODE3_ID), GRPC_PORTS.get(NODE3_ID), peersOf(gatewayIp, NODE3_ID));

        node1.start();
        node2.start();
        node3.start();
        allNodes = List.of(node1, node2, node3);
    }

    @AfterAll
    static void stopCluster() {
        for (GenericContainer<?> node : allNodes) {
            try {
                unpause(List.of(node)); // a failed test can leave a container frozen mid-scenario
            } catch (RuntimeException ignored) {
                // already running
            }
            node.stop();
        }
    }

    @Test
    void staleLeaderRejoinsAndHasItsDivergentSuffixTruncated() throws Exception {
        // --- 1. One entry committed by everyone: the common prefix the two logs will agree on.
        // Every POST here follows 409 redirects, since leadership can legitimately change under
        // us at any point; the leader is re-resolved afterwards rather than assumed.
        StateResponse initialLeaderState = awaitSingleLeader(allNodes, 0);
        assertEquals(200, postCommandFollowingLeader(LogReconciliationIT::containerForNodeId,
                containerForNodeId(initialLeaderState.nodeId()), "base", "SET", 1).statusCode());

        StateResponse oldLeaderState = awaitSingleLeader(allNodes, 0);
        GenericContainer<?> oldLeader = containerForNodeId(oldLeaderState.nodeId());
        List<GenericContainer<?>> followers = allNodes.stream().filter(n -> n != oldLeader).toList();

        // --- 2. Partition the leader away from both followers and write to it anyway. Nothing can
        // commit without a majority, but the entries still land in the leader's own log - which is
        // exactly the divergence Raft has to clean up later. This implementation has no
        // check-quorum step-down, so the isolated node keeps believing it is leader.
        List<String> ghostKeys = List.of("ghost1", "ghost2");
        pause(followers);
        try {
            // With two of three nodes frozen no election can happen any more, so this one check is
            // enough to rule out a leadership change having slipped in just before the pause.
            assertEquals("LEADER", state(oldLeader).status(), "leadership changed before the partition");

            for (String key : ghostKeys) {
                // 504: HttpApi gives up waiting for the commit after 5s.
                assertEquals(504, postCommand(oldLeader, key, "SET", 1).statusCode());
            }

            LogResponse diverged = getLog(oldLeader);
            assertEquals(List.of("base", "ghost1", "ghost2"), keysOf(diverged));
            assertEquals(1, diverged.commitIndex(), "the ghost entries must not have committed");
        } finally {
            // Pause the old leader *before* reviving the followers - otherwise its heartbeats keep
            // reaching them and they never hold an election.
            pause(List.of(oldLeader));
            unpause(followers);
        }

        // --- 3. The two survivors elect a new leader (their logs are identical, so either wins)
        // and commit three real entries over the indexes the ghosts occupy on the stale node.
        StateResponse firstNewLeaderState = awaitSingleLeader(followers, oldLeaderState.currentTerm());
        for (String key : List.of("real1", "real2", "real3")) {
            assertEquals(200, postCommandFollowingLeader(LogReconciliationIT::containerForNodeId,
                    containerForNodeId(firstNewLeaderState.nodeId()), key, "SET", 1).statusCode());
        }

        StateResponse newLeaderState = awaitSingleLeader(followers, oldLeaderState.currentTerm());
        GenericContainer<?> newLeader = containerForNodeId(newLeaderState.nodeId());
        assertEquals(List.of("base", "real1", "real2", "real3"), keysOf(getLog(newLeader)));

        // --- 3b. Force one more election, now that the log is four entries long, so that whoever
        // leads next initializes nextIndex for the still-absent node to 4 + 1 = 5. Pausing the
        // leader is enough: the lone survivor can't reach a majority on its own, but it keeps
        // bumping its term, so an election resolves as soon as the leader is back.
        pause(List.of(newLeader));
        Thread.sleep(1_000);
        unpause(List.of(newLeader));
        StateResponse optimisticLeaderState = awaitSingleLeader(followers, newLeaderState.currentTerm());
        LogResponse authoritative = getLog(containerForNodeId(optimisticLeaderState.nodeId()));
        assertEquals(List.of("base", "real1", "real2", "real3"), keysOf(authoritative));

        // --- 4. Bring the stale node back. Nothing else to do: the leader's next heartbeat starts
        // the backtracking, and within a few round trips the ghosts are gone.
        unpause(List.of(oldLeader));

        // Generous timeout: a resumed stale leader can trigger a disruptive election or two before
        // settling down. It can never win one (its last log term is behind, so the §5.4.1 election
        // restriction denies it), and committed entries never change, so the expectation holds
        // however many elections happen in between.
        //
        // Only the log is asserted, deliberately. commitIndex is not a stable expectation here:
        // advanceCommitIndex() refuses to commit entries from earlier terms by counting replicas
        // (§5.4.2), and every entry here predates the current term, so a leader elected after the
        // last write stays at whatever commitIndex it carried over until some new command arrives.
        // That's correct Raft; the paper's remedy (a no-op entry appended on election) isn't
        // implemented. Log reconciliation is what this test is about, and it doesn't depend on it.
        await().atMost(Duration.ofSeconds(30)).ignoreExceptions().untilAsserted(() ->
                assertEquals(authoritative.log(), getLog(oldLeader).log(),
                        "stale node's log should match the leader's exactly"));

        // The ghosts were never committed, so they were never applied to the state machine either.
        Map<String, Integer> stateMachine = getStateMachine(oldLeader).state();
        assertTrue(stateMachine.containsKey("base"));
        ghostKeys.forEach(key -> assertFalse(stateMachine.containsKey(key), key + " should never have been applied"));
    }

    private static List<String> keysOf(LogResponse log) {
        return log.log().stream().map(entry -> entry.command().key()).toList();
    }

    /** The `PEERS` value for {@code nodeId}: every other node in the cluster. */
    private static String peersOf(String gatewayIp, int nodeId) {
        return HTTP_PORTS.keySet().stream()
                .filter(peerId -> peerId != nodeId)
                .sorted()
                .map(peerId -> peerEntry(gatewayIp, peerId, GRPC_PORTS.get(peerId), HTTP_PORTS.get(peerId)))
                .reduce((a, b) -> a + "," + b)
                .orElseThrow();
    }

    private static GenericContainer<?> containerForNodeId(int nodeId) {
        return switch (nodeId) {
            case NODE1_ID -> node1;
            case NODE2_ID -> node2;
            case NODE3_ID -> node3;
            default -> throw new IllegalArgumentException("Unknown node id: " + nodeId);
        };
    }
}
