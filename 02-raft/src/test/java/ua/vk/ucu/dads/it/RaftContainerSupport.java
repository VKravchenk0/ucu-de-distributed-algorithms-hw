package ua.vk.ucu.dads.it;

import org.testcontainers.containers.FixedHostPortGenericContainer;
import org.testcontainers.containers.GenericContainer;
import org.testcontainers.images.builder.ImageFromDockerfile;

import java.nio.file.Paths;
import java.time.Duration;

import static org.testcontainers.containers.wait.strategy.Wait.forHttp;

/**
 * Two networking quirks of this sandboxed devcontainer, verified by hand before landing on
 * this design: (1) ports published from a container on a Testcontainers-managed custom
 * {@code Network} are not reachable from the test JVM (even plain nginx wasn't reachable
 * there), so nodes stay on the default bridge network and secondaries are addressed by raw
 * bridge IP (see {@link #containerAddress}) rather than a Docker DNS alias. (2) Even on the
 * default bridge, only explicitly-fixed host ports are reachable from the test JVM -
 * Docker's normal dynamically-assigned host ports are not, seemingly because this sandbox's
 * outer port-forwarding only registers ports it sees requested explicitly. Hence
 * {@link FixedHostPortGenericContainer} instead of plain {@code withExposedPorts}.
 */
public abstract class RaftContainerSupport {
    protected static final String IMAGE_NAME = "raft-node:test";
    protected static final int HTTP_PORT = 7000;
    protected static final int GRPC_PORT = 6001;

    private static final ImageFromDockerfile IMAGE = new ImageFromDockerfile(IMAGE_NAME, false)
            .withFileFromPath("pom.xml", Paths.get("pom.xml"))
            .withFileFromPath("src", Paths.get("src"))
            .withFileFromPath("Dockerfile", Paths.get("Dockerfile"));

    static {
        IMAGE.get();
    }

    @SuppressWarnings("deprecation")
    protected static GenericContainer<?> newNode(int hostPort) {
        return new FixedHostPortGenericContainer<>(IMAGE_NAME)
                .withFixedExposedPort(hostPort, HTTP_PORT)
                .waitingFor(forHttp("/health").forStatusCode(200).withStartupTimeout(Duration.ofSeconds(60)));
    }

    protected static String baseUrl(GenericContainer<?> container) {
        return "http://" + container.getHost() + ":" + container.getMappedPort(HTTP_PORT);
    }

    protected static String containerAddress(GenericContainer<?> container) {
        String ip = container.getContainerInfo().getNetworkSettings().getNetworks().get("bridge").getIpAddress();
        return ip + ":" + GRPC_PORT;
    }
}
