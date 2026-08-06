package ua.vk.ucu.dads.it;

import com.github.dockerjava.api.DockerClient;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.testcontainers.containers.GenericContainer;

import java.net.URI;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.List;
import java.util.Map;

import static org.awaitility.Awaitility.await;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ReplicationIT extends RaftContainerSupport {

    protected record StateMachineResponse(int nodeId, String status, int commitIndex, int lastApplied, Map<String, Integer> state) {
    }

    private static final int NODE1_ID = 1;
    private static final int NODE2_ID = 2;
    private static final int NODE3_ID = 3;
    private static final Map<Integer, Integer> HTTP_PORTS = Map.of(NODE1_ID, 17000, NODE2_ID, 17001, NODE3_ID, 17002);

    static GenericContainer<?> node1;
    static GenericContainer<?> node2;
    static GenericContainer<?> node3;
    static List<GenericContainer<?>> allNodes;
    static String gatewayIp;

    @BeforeAll
    static void startCluster() {
        gatewayIp = bridgeGatewayIp();

        node1 = newRaftNode(NODE1_ID, HTTP_PORTS.get(NODE1_ID), 16001,
                peerEntry(gatewayIp, NODE2_ID, 16002, HTTP_PORTS.get(NODE2_ID)) + ","
                        + peerEntry(gatewayIp, NODE3_ID, 16003, HTTP_PORTS.get(NODE3_ID)));
        node2 = newRaftNode(NODE2_ID, HTTP_PORTS.get(NODE2_ID), 16002,
                peerEntry(gatewayIp, NODE1_ID, 16001, HTTP_PORTS.get(NODE1_ID)) + ","
                        + peerEntry(gatewayIp, NODE3_ID, 16003, HTTP_PORTS.get(NODE3_ID)));
        node3 = newRaftNode(NODE3_ID, HTTP_PORTS.get(NODE3_ID), 16003,
                peerEntry(gatewayIp, NODE1_ID, 16001, HTTP_PORTS.get(NODE1_ID)) + ","
                        + peerEntry(gatewayIp, NODE2_ID, 16002, HTTP_PORTS.get(NODE2_ID)));

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
    void commandsCommitAndReplicateStateMachineAcrossCluster() throws Exception {
        StateResponse leaderState = awaitSingleLeader(allNodes, 0);
        GenericContainer<?> leader = containerForNodeId(leaderState.nodeId());

        assertEquals(200, postCommand(leader, "x", "SET", 5).statusCode());
        HttpResponse<String> addResponse = postCommand(leader, "x", "ADD", 2);
        assertEquals(200, addResponse.statusCode());
        assertTrue(addResponse.body().contains("\"value\":7"));

        await().atMost(Duration.ofSeconds(20)).untilAsserted(() -> {
            for (GenericContainer<?> node : allNodes) {
                assertEquals(Integer.valueOf(7), getStateMachine(node).state().get("x"));
            }
        });
    }

    @Test
    void postToNonLeaderReturnsRedirectToActualLeader() throws Exception {
        StateResponse leaderState = awaitSingleLeader(allNodes, 0);
        GenericContainer<?> leader = containerForNodeId(leaderState.nodeId());
        GenericContainer<?> nonLeader = allNodes.stream().filter(n -> n != leader).findFirst().orElseThrow();

        HttpResponse<String> response = postCommand(nonLeader, "y", "SET", 1);

        assertEquals(409, response.statusCode());
        assertTrue(response.body().contains("redirect"));
        String expectedLeaderUrl = "http://" + gatewayIp + ":" + HTTP_PORTS.get(leaderState.nodeId());
        assertTrue(response.body().contains(expectedLeaderUrl));
    }

    @Test
    void addOnUnknownKeyReturnsErrorAndDoesNotMutateAnyReplica() throws Exception {
        StateResponse leaderState = awaitSingleLeader(allNodes, 0);
        GenericContainer<?> leader = containerForNodeId(leaderState.nodeId());

        HttpResponse<String> response = postCommand(leader, "never-set", "ADD", 1);

        assertEquals(400, response.statusCode());
        assertTrue(response.body().contains("error"));

        // The POST already waited for commit+apply on the leader; give the (harmless, since it
        // failed deterministically everywhere) replication a moment to reach the followers too.
        Thread.sleep(500);
        for (GenericContainer<?> node : allNodes) {
            assertFalse(getStateMachine(node).state().containsKey("never-set"));
        }
    }

    @Test
    void pausedFollowerCatchesUpAfterResume() throws Exception {
        StateResponse leaderState = awaitSingleLeader(allNodes, 0);
        GenericContainer<?> leader = containerForNodeId(leaderState.nodeId());
        GenericContainer<?> follower = allNodes.stream().filter(n -> n != leader).findFirst().orElseThrow();

        DockerClient dockerClient = follower.getDockerClient();
        dockerClient.pauseContainerCmd(follower.getContainerId()).exec();
        try {
            // Leader + the one remaining live follower is still a majority, so these commit
            // even though `follower` cannot acknowledge anything while paused. `leader` was
            // resolved a moment ago and, under load, may have since stepped down in a
            // legitimate election unrelated to the pause above - follow the 409 redirect like
            // a real client would rather than assume that snapshot is still current.
            assertEquals(200, postCommandFollowingLeader(leader, "z", "SET", 1).statusCode());
            assertEquals(200, postCommandFollowingLeader(leader, "z", "ADD", 1).statusCode());
            assertEquals(200, postCommandFollowingLeader(leader, "z", "ADD", 1).statusCode());
        } finally {
            dockerClient.unpauseContainerCmd(follower.getContainerId()).exec();
        }

        await().atMost(Duration.ofSeconds(20)).untilAsserted(() ->
                assertEquals(Integer.valueOf(3), getStateMachine(follower).state().get("z")));
    }

    protected record RedirectResponse(String status, Integer leaderId) {
    }

    /**
     * Like {@link #postCommand}, but follows a 409's leader redirect instead of assuming
     * {@code node} is still leader - under load the cluster can flap through a few elections in
     * a row, so this keeps following redirects/retrying against a time budget rather than a
     * fixed attempt count.
     */
    private static HttpResponse<String> postCommandFollowingLeader(GenericContainer<?> node, String key, String action, int value) throws Exception {
        GenericContainer<?> target = node;
        long deadline = System.nanoTime() + Duration.ofSeconds(10).toNanos();
        while (true) {
            HttpResponse<String> response = postCommand(target, key, action, value);
            if (response.statusCode() != 409 || System.nanoTime() >= deadline) {
                return response;
            }
            RedirectResponse redirect = MAPPER.readValue(response.body(), RedirectResponse.class);
            if (redirect.leaderId() == null) {
                Thread.sleep(50);
                continue;
            }
            target = containerForNodeId(redirect.leaderId());
        }
    }

    private static HttpResponse<String> postCommand(GenericContainer<?> node, String key, String action, int value) throws Exception {
        String body = String.format("{\"key\":\"%s\",\"action\":\"%s\",\"value\":%d}", key, action, value);
        HttpRequest post = HttpRequest.newBuilder(URI.create(baseUrl(node) + "/command"))
                .header("Content-Type", "application/json")
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build();
        return HTTP.send(post, HttpResponse.BodyHandlers.ofString());
    }

    private static StateMachineResponse getStateMachine(GenericContainer<?> node) throws Exception {
        HttpRequest get = HttpRequest.newBuilder(URI.create(baseUrl(node) + "/state-machine")).GET().build();
        HttpResponse<String> resp = HTTP.send(get, HttpResponse.BodyHandlers.ofString());
        return MAPPER.readValue(resp.body(), StateMachineResponse.class);
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
