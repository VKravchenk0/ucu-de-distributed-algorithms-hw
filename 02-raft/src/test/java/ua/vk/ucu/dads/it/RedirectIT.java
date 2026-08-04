package ua.vk.ucu.dads.it;

import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.testcontainers.containers.GenericContainer;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class RedirectIT extends RaftContainerSupport {

    static GenericContainer<?> master;
    static GenericContainer<?> secondary;
    static final HttpClient http = HttpClient.newHttpClient();

    @BeforeAll
    static void startCluster() {
        secondary = newNode(17001)
                .withEnv("IS_MASTER", "false")
                .withEnv("MASTER_URL", "http://master:" + HTTP_PORT);
        secondary.start();

        master = newNode(17000)
                .withEnv("IS_MASTER", "true")
                .withEnv("SECONDARY_ADDRESSES", containerAddress(secondary));
        master.start();
    }

    @AfterAll
    static void stopCluster() {
        secondary.stop();
        master.stop();
    }

    @Test
    void postToSecondaryReturnsMasterUrlInsteadOfReplicating() throws Exception {
        HttpRequest post = HttpRequest.newBuilder(URI.create(baseUrl(secondary) + "/log"))
                .header("Content-Type", "application/json")
                .POST(HttpRequest.BodyPublishers.ofString("{\"message\":\"should-not-replicate\"}"))
                .build();
        HttpResponse<String> response = http.send(post, HttpResponse.BodyHandlers.ofString());

        assertEquals(200, response.statusCode());
        assertTrue(response.body().contains("http://master:" + HTTP_PORT));
        assertTrue(response.body().contains("redirect"));

        HttpRequest getSecondaryLog = HttpRequest.newBuilder(URI.create(baseUrl(secondary) + "/log")).GET().build();
        HttpResponse<String> secondaryLog = http.send(getSecondaryLog, HttpResponse.BodyHandlers.ofString());
        assertEquals("[]", secondaryLog.body().trim());
    }
}
