package ua.vk.ucu.dads.it;

import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.testcontainers.containers.GenericContainer;
import ua.vk.ucu.dads.log.LogEntry;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.time.Duration;
import java.util.List;

import static org.awaitility.Awaitility.await;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ReplicationIT extends RaftContainerSupport {

    static GenericContainer<?> master;
    static GenericContainer<?> secondary1;
    static GenericContainer<?> secondary2;

    static final HttpClient http = HttpClient.newHttpClient();
    static final ObjectMapper mapper = new ObjectMapper();

    @BeforeAll
    static void startCluster() {
        secondary1 = newNode(17001)
                .withEnv("IS_MASTER", "false")
                .withEnv("MASTER_URL", "http://master:" + HTTP_PORT);
        secondary2 = newNode(17002)
                .withEnv("IS_MASTER", "false")
                .withEnv("MASTER_URL", "http://master:" + HTTP_PORT);
        secondary1.start();
        secondary2.start();

        String secondaryAddresses = containerAddress(secondary1) + "," + containerAddress(secondary2);
        master = newNode(17000)
                .withEnv("IS_MASTER", "true")
                .withEnv("SECONDARY_ADDRESSES", secondaryAddresses);
        master.start();
    }

    @AfterAll
    static void stopCluster() {
        master.stop();
        secondary1.stop();
        secondary2.stop();
    }

    @Test
    void messagePostedToMasterReplicatesToAllNodes() throws Exception {
        String messageText = "hello-raft-" + System.currentTimeMillis();

        HttpRequest post = HttpRequest.newBuilder(URI.create(baseUrl(master) + "/log"))
                .header("Content-Type", "application/json")
                .POST(HttpRequest.BodyPublishers.ofString("{\"message\":\"" + messageText + "\"}"))
                .build();
        HttpResponse<String> response = http.send(post, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, response.statusCode());

        List<GenericContainer<?>> allNodes = List.of(master, secondary1, secondary2);
        await().atMost(Duration.ofSeconds(20)).untilAsserted(() -> {
            for (GenericContainer<?> node : allNodes) {
                assertTrue(getLog(node).stream().anyMatch(e -> e.message().equals(messageText)));
            }
        });
    }

    private static List<LogEntry> getLog(GenericContainer<?> node) throws Exception {
        HttpRequest get = HttpRequest.newBuilder(URI.create(baseUrl(node) + "/log")).GET().build();
        HttpResponse<String> resp = http.send(get, HttpResponse.BodyHandlers.ofString());
        return mapper.readValue(resp.body(), mapper.getTypeFactory().constructCollectionType(List.class, LogEntry.class));
    }
}
