package ua.vk.ucu.dads.it;

import com.github.dockerjava.api.DockerClient;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.testcontainers.containers.GenericContainer;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests election + failover: a 3-node cluster elects a single leader, the leader is
 * paused so the remaining majority elects a new leader at a
 * higher term, then the old leader is unpaused and must recognize the new leader and step down,
 * leaving exactly one leader again.
 */
class LeaderElectionIT extends RaftTestSupport {

    private static final int NODE1_ID = 1;
    private static final int NODE2_ID = 2;
    private static final int NODE3_ID = 3;

    static GenericContainer<?> node1;
    static GenericContainer<?> node2;
    static GenericContainer<?> node3;
    static List<GenericContainer<?>> allNodes;

    @BeforeAll
    static void startCluster() {
        String gw = bridgeGatewayIp();

        node1 = newRaftNode(NODE1_ID, 17010, 16010,
                peerEntry(gw, NODE2_ID, 16011, 17011) + "," + peerEntry(gw, NODE3_ID, 16012, 17012));
        node2 = newRaftNode(NODE2_ID, 17011, 16011,
                peerEntry(gw, NODE1_ID, 16010, 17010) + "," + peerEntry(gw, NODE3_ID, 16012, 17012));
        node3 = newRaftNode(NODE3_ID, 17012, 16012,
                peerEntry(gw, NODE1_ID, 16010, 17010) + "," + peerEntry(gw, NODE2_ID, 16011, 17011));

        node1.start();
        node2.start();
        node3.start();
        allNodes = List.of(node1, node2, node3);
    }

    @AfterAll
    static void stopCluster() {
        node1.stop();
        node2.stop();
        node3.stop();
    }

    @Test
    void clusterElectsNewLeaderAfterPauseAndOldLeaderStepsDownOnResume() {
        StateResponse initialLeader = awaitSingleLeader(allNodes, 0);
        assertTrue(initialLeader.currentTerm() > 0);

        GenericContainer<?> leaderContainer = containerForNodeId(initialLeader.nodeId());
        DockerClient dockerClient = leaderContainer.getDockerClient();
        dockerClient.pauseContainerCmd(leaderContainer.getContainerId()).exec();

        try {
            List<GenericContainer<?>> remaining = allNodes.stream()
                    .filter(n -> n != leaderContainer)
                    .toList();
            StateResponse newLeader = awaitSingleLeader(remaining, initialLeader.currentTerm());
            assertNotEquals(initialLeader.nodeId(), newLeader.nodeId());
        } finally {
            dockerClient.unpauseContainerCmd(leaderContainer.getContainerId()).exec();
        }

        awaitSingleLeader(allNodes, initialLeader.currentTerm());
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
