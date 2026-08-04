package ua.vk.ucu.dads.it;

import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.testcontainers.containers.GenericContainer;
import ua.vk.ucu.dads.log.LogEntry;

import java.net.URI;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.List;
import java.util.Map;

import static org.awaitility.Awaitility.await;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ReplicationIT extends RaftContainerSupport {

    protected record LogResponse(int nodeId, int currentTerm, String status, List<LogEntry> log) {
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
    void messagePostedToLeaderReplicatesToAllNodes() throws Exception {
        StateResponse leaderState = awaitSingleLeader(allNodes, 0);
        GenericContainer<?> leader = containerForNodeId(leaderState.nodeId());

        String messageText = "hello-raft-" + System.currentTimeMillis();
        HttpRequest post = HttpRequest.newBuilder(URI.create(baseUrl(leader) + "/log"))
                .header("Content-Type", "application/json")
                .POST(HttpRequest.BodyPublishers.ofString("{\"message\":\"" + messageText + "\"}"))
                .build();
        HttpResponse<String> response = HTTP.send(post, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, response.statusCode());

        await().atMost(Duration.ofSeconds(20)).untilAsserted(() -> {
            for (GenericContainer<?> node : allNodes) {
                assertTrue(getLog(node).stream().anyMatch(e -> e.message().equals(messageText)));
            }
        });
    }

    @Test
    void postToNonLeaderReturnsRedirectToActualLeader() throws Exception {
        StateResponse leaderState = awaitSingleLeader(allNodes, 0);
        GenericContainer<?> leader = containerForNodeId(leaderState.nodeId());
        GenericContainer<?> nonLeader = allNodes.stream().filter(n -> n != leader).findFirst().orElseThrow();

        HttpRequest post = HttpRequest.newBuilder(URI.create(baseUrl(nonLeader) + "/log"))
                .header("Content-Type", "application/json")
                .POST(HttpRequest.BodyPublishers.ofString("{\"message\":\"should-not-replicate\"}"))
                .build();
        HttpResponse<String> response = HTTP.send(post, HttpResponse.BodyHandlers.ofString());

        assertEquals(409, response.statusCode());
        assertTrue(response.body().contains("redirect"));
        String expectedLeaderUrl = "http://" + gatewayIp + ":" + HTTP_PORTS.get(leaderState.nodeId());
        assertTrue(response.body().contains(expectedLeaderUrl));
    }

    private static List<LogEntry> getLog(GenericContainer<?> node) throws Exception {
        HttpRequest get = HttpRequest.newBuilder(URI.create(baseUrl(node) + "/log")).GET().build();
        HttpResponse<String> resp = HTTP.send(get, HttpResponse.BodyHandlers.ofString());
        return MAPPER.readValue(resp.body(), LogResponse.class).log();
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
