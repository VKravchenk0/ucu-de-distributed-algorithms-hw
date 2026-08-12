package ua.vk.ucu.dads.it;

import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.testcontainers.containers.GenericContainer;

import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.List;
import java.util.Map;

import static org.awaitility.Awaitility.await;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * <pre>
 *  1. start 2 nodes                  -> a leader is elected
 *  2. post msg1, msg2                -> replicated and committed
 *  3. start the 3rd node             -> msg1, msg2 replicated onto it too
 *  4. partition the leader           -> the remaining majority elects a NewLeader
 *  5. post msg3, msg4 via NewLeader  -> replicated and committed
 *  6. post msg5 via OldLeader        -> never commits (it has no majority)
 *  7. heal the partition             -> msg5 is replaced by the NewLeader's msg3, msg4
 * </pre>
 */
class AcceptanceTestIT extends RaftTestSupport {

    private static final int NODE1_ID = 1;
    private static final int NODE2_ID = 2;
    private static final int NODE3_ID = 3;

    private static final Map<Integer, Integer> HTTP_PORTS = Map.of(
        NODE1_ID, 17020, 
        NODE2_ID, 17021, 
        NODE3_ID, 17022
    );

    private static final Map<Integer, Integer> GRPC_PORTS = Map.of(
        NODE1_ID, 16020, 
        NODE2_ID, 16021, 
        NODE3_ID, 16022
    );

    private static final Map<String, Integer> AFTER_MSG2 = Map.of("msg1", 1, "msg2", 2);
    private static final Map<String, Integer> AFTER_MSG4 = Map.of("msg1", 1, "msg2", 2, "msg3", 3, "msg4", 4);

    static GenericContainer<?> node1;
    static GenericContainer<?> node2;
    static GenericContainer<?> node3;
    static List<GenericContainer<?>> allNodes;

    @BeforeAll
    static void defineCluster() {
        String gatewayIp = bridgeGatewayIp();
        node1 = newRaftNode(NODE1_ID, HTTP_PORTS.get(NODE1_ID), GRPC_PORTS.get(NODE1_ID), peersOf(gatewayIp, NODE1_ID));
        node2 = newRaftNode(NODE2_ID, HTTP_PORTS.get(NODE2_ID), GRPC_PORTS.get(NODE2_ID), peersOf(gatewayIp, NODE2_ID));
        node3 = newRaftNode(NODE3_ID, HTTP_PORTS.get(NODE3_ID), GRPC_PORTS.get(NODE3_ID), peersOf(gatewayIp, NODE3_ID));
        allNodes = List.of(node1, node2, node3);
    }

    @AfterAll
    static void stopCluster() {
        for (GenericContainer<?> node : allNodes) {
            if (node.getContainerId() == null) {
                continue; // never started (e.g. the test failed before step 3)
            }
            try {
                unpause(List.of(node)); // a failed test can leave a container frozen mid-scenario
            } catch (RuntimeException ignored) {
                // already running
            }
            node.stop();
        }
    }

    @Test
    void twoNodesElectCommitAndSurviveALeaderPartition() throws Exception {
        // --- 1. Two of the three members are up, waiting for a leader to be elected
        node1.start();
        node2.start();
        List<GenericContainer<?>> firstTwo = List.of(node1, node2);
        StateResponse firstLeaderState = awaitSingleLeader(firstTwo, 0);
        assertTrue(firstLeaderState.currentTerm() > 0, "a leader must have been elected in some term > 0");

        // --- 2. Sending two commands
        postMessages(firstLeaderState.nodeId(), 1, 2);
        awaitConverged(firstTwo, List.of("msg1", "msg2"), 2, AFTER_MSG2);

        // --- 3. The third member finally boots. The leader has been retrying AppendEntries against
        // it since it was elected
        node3.start();
        awaitConverged(allNodes, List.of("msg1", "msg2"), 2, AFTER_MSG2);

        // --- 4. Partition the leader away from the other two. The survivors are still a majority,
        // so they elect a new leader at a higher term.
        StateResponse oldLeaderState = awaitSingleLeader(allNodes, 0);
        GenericContainer<?> oldLeader = containerForNodeId(oldLeaderState.nodeId());
        List<GenericContainer<?>> majority = allNodes.stream().filter(node -> node != oldLeader).toList();

        pause(List.of(oldLeader));
        StateResponse newLeaderState = awaitSingleLeader(majority, oldLeaderState.currentTerm());
        assertNotEquals(oldLeaderState.nodeId(), newLeaderState.nodeId());

        // --- 5. Two more commands via the new leader
        postMessages(newLeaderState.nodeId(), 3, 4);
        awaitConverged(majority, List.of("msg1", "msg2", "msg3", "msg4"), 4, AFTER_MSG4);

        // --- 6. Now flip which side of the partition is frozen, so that the still-isolated
        // OldLeader can be written to. It cannot reach anyone, so msg5 lands in its log and stays
        // there uncommitted until the HTTP layer gives up waiting after 5s.
        pause(majority);
        unpause(List.of(oldLeader));
        HttpResponse<String> msg5Response = postCommand(oldLeader, "msg5", "SET", 5);

        if (msg5Response.statusCode() == 409) {
            // Not a leader: only reachable if the OldLeader had already been
            // deposed in the instant between step 4 resolving it and
            // pausing it, in which case there is no msg5 anywhere and step 7 is a no-op.
            assertTrue(msg5Response.body().contains("redirect") || msg5Response.body().contains("no-leader"));
        } else {
            // Message 5 not commited
            assertEquals(504, msg5Response.statusCode(), "msg5 must neither commit nor be rejected outright");
            LogResponse diverged = getLog(oldLeader);
            assertEquals(List.of("msg1", "msg2", "msg5"), keysOf(diverged), "msg5 should sit in the OldLeader's log");
            assertEquals(2, diverged.commitIndex(), "msg5 must not have committed without a majority");
            assertEquals(AFTER_MSG2, getStateMachine(oldLeader).state(), "uncommitted entries are never applied");
        }

        // --- 7. Heal the partition. The leader's AppendEntries back up until
        // they find the last agreeing index and then overwrite the OldLeader's divergent suffix.
        unpause(majority);
        awaitConverged(allNodes, List.of("msg1", "msg2", "msg3", "msg4"), 4, AFTER_MSG4);

        // Ensuring the old leader is not the current leader
        assertNotEquals("LEADER", state(oldLeader).status());
    }

    private static void postMessages(int preferredLeaderId, int firstIndex, int lastIndex) throws Exception {
        for (int i = firstIndex; i <= lastIndex; i++) {
            HttpResponse<String> response = postCommandFollowingLeader(AcceptanceTestIT::containerForNodeId,
                    containerForNodeId(preferredLeaderId), "msg" + i, "SET", i);
            assertEquals(200, response.statusCode(), "msg" + i + " should have committed: " + response.body());
        }
    }

    /**
     * Waits until every one of {@code nodes} holds exactly {@code expectedKeys} in its log, has
     * committed all of it, and has applied it to the same state machine contents.
     */
    private static void awaitConverged(List<GenericContainer<?>> nodes, List<String> expectedKeys,
                                       int expectedCommitIndex, Map<String, Integer> expectedState) {
        await().atMost(Duration.ofSeconds(40)).ignoreExceptions().untilAsserted(() -> {
            for (GenericContainer<?> node : nodes) {
                LogResponse log = getLog(node);
                assertEquals(expectedKeys, keysOf(log), "log on node " + log.nodeId());
                assertEquals(expectedCommitIndex, log.commitIndex(), "commitIndex on node " + log.nodeId());
                assertEquals(expectedState, getStateMachine(node).state(), "state machine on node " + log.nodeId());
            }
        });
    }

    private static List<String> keysOf(LogResponse log) {
        return log.log().stream().map(entry -> entry.command().key()).toList();
    }

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
