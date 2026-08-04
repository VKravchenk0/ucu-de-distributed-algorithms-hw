package ua.vk.ucu.dads.it;

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
import java.util.concurrent.atomic.AtomicReference;

import static org.awaitility.Awaitility.await;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.testcontainers.containers.wait.strategy.Wait.forHttp;

/**
 * Two networking quirks of this sandboxed devcontainer, verified by hand before landing on
 * this design: (1) ports published from a container on a Testcontainers-managed custom
 * {@code Network} are not reachable from the test JVM (even plain nginx wasn't reachable
 * there), so nodes stay on the default bridge network. (2) Even on the default bridge, only
 * explicitly-fixed host ports are reachable - Docker's normal dynamically-assigned host ports
 * are not, seemingly because this sandbox's outer port-forwarding only registers ports it sees
 * requested explicitly. Hence {@link FixedHostPortGenericContainer} instead of plain
 * {@code withExposedPorts}.
 *
 * <p>Raft peer discovery is fully symmetric (every node needs every other node's address at
 * boot), unlike the old hub-and-spoke master/secondary model where only the master needed to
 * know secondary addresses. That rules out addressing peers by their bridge-network IP, since
 * a container's IP isn't known until it starts, and a static peer list would need every node's
 * address before any node starts. Instead, each node fixes both its HTTP and gRPC ports to
 * host ports chosen up front by the test ({@link #newRaftNode}), and peers address each other
 * via the Docker bridge gateway IP plus the peer's fixed host port ({@link #bridgeGatewayIp()})
 * - the same "reach a container via an explicitly-fixed host port" mechanism already proven to
 * work in this sandbox for quirk (2) above, just used container-to-container instead of from
 * the test JVM.
 */
public abstract class RaftContainerSupport {
    protected static final String IMAGE_NAME = "raft-node:test";
    protected static final int HTTP_PORT = 7000;
    protected static final int GRPC_PORT = 6001;

    protected static final HttpClient HTTP = HttpClient.newHttpClient();
    protected static final ObjectMapper MAPPER = new ObjectMapper();

    private static final ImageFromDockerfile IMAGE = new ImageFromDockerfile(IMAGE_NAME, false)
            .withFileFromPath("pom.xml", Paths.get("pom.xml"))
            .withFileFromPath("src", Paths.get("src"))
            .withFileFromPath("Dockerfile", Paths.get("Dockerfile"));

    static {
        IMAGE.get();
    }

    protected record StateResponse(int nodeId, int currentTerm, String status, Integer leaderId) {
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

    /** Polls {@code nodes} until exactly one reports LEADER with a term strictly greater than {@code mustExceedTerm}. */
    protected static StateResponse awaitSingleLeader(List<GenericContainer<?>> nodes, int mustExceedTerm) {
        AtomicReference<StateResponse> leader = new AtomicReference<>();
        await().atMost(Duration.ofSeconds(20)).untilAsserted(() -> {
            List<StateResponse> states = nodes.stream().map(RaftContainerSupport::state).toList();
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
