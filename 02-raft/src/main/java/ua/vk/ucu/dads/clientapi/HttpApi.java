package ua.vk.ucu.dads.clientapi;

import io.javalin.Javalin;
import ua.vk.ucu.dads.config.NodeConfig;
import ua.vk.ucu.dads.log.LogEntry;
import ua.vk.ucu.dads.log.LogStore;
import ua.vk.ucu.dads.raft.RaftNode;
import ua.vk.ucu.dads.raft.ServerState;
import ua.vk.ucu.dads.raft.ServerStatus;
import ua.vk.ucu.dads.replication.ReplicationClient;

import java.util.LinkedHashMap;
import java.util.Map;

public class HttpApi {
    private final RaftNode raftNode;
    private final NodeConfig config;
    private final LogStore logStore;
    private final ReplicationClient replicationClient;

    public HttpApi(RaftNode raftNode, NodeConfig config, LogStore logStore, ReplicationClient replicationClient) {
        this.raftNode = raftNode;
        this.config = config;
        this.logStore = logStore;
        this.replicationClient = replicationClient;
    }

    public void register(Javalin app) {
        app.get("/health", ctx -> ctx.status(200));

        app.get("/state", ctx -> ctx.json(raftNode.currentState()));

        app.get("/log", ctx -> {
            ServerState state = raftNode.currentState();
            Map<String, Object> body = new LinkedHashMap<>();
            body.put("nodeId", state.nodeId());
            body.put("currentTerm", state.currentTerm());
            body.put("status", state.status());
            body.put("log", logStore.getAll());
            ctx.json(body);
        });

        app.post("/log", ctx -> {
            LogEntry incoming = ctx.bodyAsClass(LogEntry.class);
            ServerState state = raftNode.currentState();
            if (state.status() == ServerStatus.LEADER) {
                logStore.append(incoming);
                replicationClient.replicateToAllSecondaries(state.currentTerm(), state.nodeId(), incoming.message());
                ctx.json(Map.of("status", "replicated"));
            } else {
                ctx.status(409);
                ctx.json(notLeaderResponse(state.leaderId()));
            }
        });

        app.exception(Exception.class, (e, ctx) -> {
            ctx.status(500);
            ctx.json(Map.of("status", "error", "message", String.valueOf(e.getMessage())));
        });
    }

    private Map<String, Object> notLeaderResponse(Integer leaderId) {
        Map<String, Object> body = new LinkedHashMap<>();
        NodeConfig.PeerInfo leader = leaderId == null ? null : config.peers().get(leaderId);
        if (leader != null) {
            body.put("status", "redirect");
            body.put("leaderId", leaderId);
            body.put("leaderUrl", leader.httpAddress());
        } else {
            body.put("status", "no-leader");
        }
        return body;
    }
}
