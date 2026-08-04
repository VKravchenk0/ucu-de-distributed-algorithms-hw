package ua.vk.ucu.dads;

import io.javalin.Javalin;
import ua.vk.ucu.dads.replication.ReplicationClient;

import java.util.Map;

public class HttpApi {
    private final NodeConfig config;
    private final LogStore logStore;
    private final ReplicationClient replicationClient;

    public HttpApi(NodeConfig config, LogStore logStore, ReplicationClient replicationClient) {
        this.config = config;
        this.logStore = logStore;
        this.replicationClient = replicationClient;
    }

    public void register(Javalin app) {
        app.get("/health", ctx -> ctx.status(200));

        app.get("/log", ctx -> ctx.json(logStore.getAll()));

        app.post("/log", ctx -> {
            LogEntry incoming = ctx.bodyAsClass(LogEntry.class);
            if (config.isMaster()) {
                logStore.append(incoming);
                replicationClient.replicateToAllSecondaries(incoming.message());
                ctx.json(Map.of("status", "replicated"));
            } else {
                ctx.json(Map.of("status", "redirect", "masterUrl", config.masterUrl()));
            }
        });

        app.exception(Exception.class, (e, ctx) -> {
            ctx.status(500);
            ctx.json(Map.of("status", "error", "message", String.valueOf(e.getMessage())));
        });
    }
}
