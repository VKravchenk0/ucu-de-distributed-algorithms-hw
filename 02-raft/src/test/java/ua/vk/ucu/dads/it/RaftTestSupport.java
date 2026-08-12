package ua.vk.ucu.dads.it;

import com.fasterxml.jackson.databind.DeserializationFeature;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.testcontainers.DockerClientFactory;
import org.testcontainers.containers.FixedHostPortGenericContainer;
import org.testcontainers.containers.GenericContainer;
import org.testcontainers.images.builder.ImageFromDockerfile;

import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.file.Paths;
import java.time.Duration;
import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.IntFunction;

import static org.awaitility.Awaitility.await;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.testcontainers.containers.wait.strategy.Wait.forHttp;

public abstract class RaftTestSupport {
    protected static final String IMAGE_NAME = "raft-node:test";
    protected static final int HTTP_PORT = 7000;
    protected static final int GRPC_PORT = 6001;

    protected static final HttpClient HTTP = HttpClient.newHttpClient();
    // Ignore unknown JSON properties so response records here don't need to track every field
    // RaftNode's ServerState/JSON endpoints happen to expose (e.g. commitIndex, lastApplied).
    protected static final ObjectMapper MAPPER = new ObjectMapper()
            .disable(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES);

    private static final ImageFromDockerfile IMAGE = new ImageFromDockerfile(IMAGE_NAME, false)
            .withFileFromPath("pom.xml", Paths.get("pom.xml"))
            .withFileFromPath("src", Paths.get("src"))
            .withFileFromPath("Dockerfile", Paths.get("Dockerfile"));

    static {
        IMAGE.get();
    }

    protected record StateResponse(int nodeId, int currentTerm, String status, Integer leaderId) {
    }

    /** One entry of {@code GET /log}, shaped like {@code HttpApi.describeLog()}. */
    protected record LogEntryResponse(int index, int term, CommandResponse command) {
    }

    protected record CommandResponse(String key, String action, int value) {
    }

    protected record LogResponse(int nodeId, int currentTerm, String status, int commitIndex, int lastApplied,
                                 List<LogEntryResponse> log) {
    }

    protected record StateMachineResponse(int nodeId, String status, int commitIndex, int lastApplied,
                                          Map<String, Integer> state) {
    }

    /** Body of a {@code POST /command} rejected with 409 because the node isn't the leader. */
    protected record RedirectResponse(String status, Integer leaderId) {
    }

    @SuppressWarnings("deprecation")
    protected static GenericContainer<?> newRaftNode(int nodeId, int httpHostPort, int grpcHostPort, String peersEnv) {
        return new FixedHostPortGenericContainer<>(IMAGE_NAME)
                .withFixedExposedPort(httpHostPort, HTTP_PORT)
                .withFixedExposedPort(grpcHostPort, GRPC_PORT)
                .withEnv("NODE_ID", String.valueOf(nodeId))
                .withEnv("PEERS", peersEnv)
                .waitingFor(forHttp("/health").forPort(HTTP_PORT).forStatusCode(200).withStartupTimeout(Duration.ofSeconds(60)));
    }

    protected static String baseUrl(GenericContainer<?> container) {
        return "http://" + container.getHost() + ":" + container.getMappedPort(HTTP_PORT);
    }

    /** Docker bridge network gateway IP, reachable from any sibling container on that network. */
    protected static String bridgeGatewayIp() {
        var network = DockerClientFactory.instance().client().inspectNetworkCmd().withNetworkId("bridge").exec();
        return network.getIpam().getConfig().get(0).getGateway();
    }

    /** A single `PEERS` entry: {@code id=gatewayIp:grpcHostPort:httpHostPort}. */
    protected static String peerEntry(String gatewayIp, int nodeId, int grpcHostPort, int httpHostPort) {
        return nodeId + "=" + gatewayIp + ":" + grpcHostPort + ":" + httpHostPort;
    }

    protected static StateResponse state(GenericContainer<?> node) {
        try {
            HttpRequest req = HttpRequest.newBuilder(URI.create(baseUrl(node) + "/state"))
                    .timeout(Duration.ofSeconds(2))
                    .GET()
                    .build();
            HttpResponse<String> resp = HTTP.send(req, HttpResponse.BodyHandlers.ofString());
            return MAPPER.readValue(resp.body(), StateResponse.class);
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
    }

    protected static LogResponse getLog(GenericContainer<?> node) {
        try {
            HttpRequest req = HttpRequest.newBuilder(URI.create(baseUrl(node) + "/log"))
                    .timeout(Duration.ofSeconds(2))
                    .GET()
                    .build();
            HttpResponse<String> resp = HTTP.send(req, HttpResponse.BodyHandlers.ofString());
            return MAPPER.readValue(resp.body(), LogResponse.class);
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
    }

    protected static StateMachineResponse getStateMachine(GenericContainer<?> node) {
        try {
            HttpRequest req = HttpRequest.newBuilder(URI.create(baseUrl(node) + "/state-machine"))
                    .timeout(Duration.ofSeconds(2))
                    .GET()
                    .build();
            HttpResponse<String> resp = HTTP.send(req, HttpResponse.BodyHandlers.ofString());
            return MAPPER.readValue(resp.body(), StateMachineResponse.class);
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
    }

    protected static HttpResponse<String> postCommand(GenericContainer<?> node, String key, String action, int value) throws Exception {
        String body = String.format("{\"key\":\"%s\",\"action\":\"%s\",\"value\":%d}", key, action, value);
        HttpRequest post = HttpRequest.newBuilder(URI.create(baseUrl(node) + "/command"))
                .header("Content-Type", "application/json")
                // Comfortably above HttpApi's own 5s commit timeout: this is only here so that
                // posting to a paused node fails the test instead of hanging it forever.
                .timeout(Duration.ofSeconds(20))
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build();
        return HTTP.send(post, HttpResponse.BodyHandlers.ofString());
    }

    /**
     * Like {@link #postCommand}, but follows a 409's leader redirect instead of assuming
     * {@code preferred} is still leader. Keeps following
     * redirects against a time budget rather than a fixed attempt count. {@code byNodeId} maps a
     * redirect's leaderId back to its container, since only the test knows its own topology.
     */
    protected static HttpResponse<String> postCommandFollowingLeader(IntFunction<GenericContainer<?>> byNodeId,
                                                                     GenericContainer<?> preferred,
                                                                     String key, String action, int value) throws Exception {
        GenericContainer<?> target = preferred;
        long deadline = System.nanoTime() + Duration.ofSeconds(15).toNanos();
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
            target = byNodeId.apply(redirect.leaderId());
        }
    }

    protected static void pause(List<GenericContainer<?>> nodes) {
        for (GenericContainer<?> node : nodes) {
            node.getDockerClient().pauseContainerCmd(node.getContainerId()).exec();
        }
    }

    protected static void unpause(List<GenericContainer<?>> nodes) {
        for (GenericContainer<?> node : nodes) {
            node.getDockerClient().unpauseContainerCmd(node.getContainerId()).exec();
        }
    }

    /** Polls {@code nodes} until exactly one reports LEADER with a term strictly greater than {@code mustExceedTerm}. */
    protected static StateResponse awaitSingleLeader(List<GenericContainer<?>> nodes, int mustExceedTerm) {
        AtomicReference<StateResponse> leader = new AtomicReference<>();
        await().atMost(Duration.ofSeconds(20)).untilAsserted(() -> {
            List<StateResponse> states = nodes.stream().map(RaftTestSupport::state).toList();
            List<StateResponse> leaders = states.stream()
                    .filter(s -> "LEADER".equals(s.status()))
                    .filter(s -> s.currentTerm() > mustExceedTerm)
                    .toList();
            assertEquals(1, leaders.size(), "expected exactly one leader among " + states);
            leader.set(leaders.get(0));
        });
        return leader.get();
    }
}
