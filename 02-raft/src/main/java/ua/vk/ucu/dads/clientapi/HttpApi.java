package ua.vk.ucu.dads.clientapi;

import io.javalin.Javalin;
import ua.vk.ucu.dads.config.NodeConfig;
import ua.vk.ucu.dads.raft.CommandResult;
import ua.vk.ucu.dads.raft.RaftNode;
import ua.vk.ucu.dads.raft.ServerState;
import ua.vk.ucu.dads.raft.log.LogEntry;
import ua.vk.ucu.dads.raft.log.LogStore;
import ua.vk.ucu.dads.statemachine.Command;
import ua.vk.ucu.dads.statemachine.StateMachine;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

public class HttpApi {
    private static final long COMMAND_TIMEOUT_SECONDS = 5;

    private final RaftNode raftNode;
    private final NodeConfig config;
    private final LogStore logStore;
    private final StateMachine stateMachine;

    public HttpApi(RaftNode raftNode, NodeConfig config, LogStore logStore, StateMachine stateMachine) {
        this.raftNode = raftNode;
        this.config = config;
        this.logStore = logStore;
        this.stateMachine = stateMachine;
    }

    public record CommandRequest(String key, String action, int value) {
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
            body.put("commitIndex", state.commitIndex());
            body.put("lastApplied", state.lastApplied());
            body.put("log", describeLog());
            ctx.json(body);
        });

        app.get("/state-machine", ctx -> {
            ServerState state = raftNode.currentState();
            Map<String, Object> body = new LinkedHashMap<>();
            body.put("nodeId", state.nodeId());
            body.put("status", state.status());
            body.put("commitIndex", state.commitIndex());
            body.put("lastApplied", state.lastApplied());
            body.put("state", stateMachine.snapshot());
            ctx.json(body);
        });

        app.post("/command", ctx -> {
            CommandRequest incoming = ctx.bodyAsClass(CommandRequest.class);
            Command.Action action;
            try {
                action = Command.Action.valueOf(incoming.action().toUpperCase());
            } catch (IllegalArgumentException e) {
                ctx.status(400);
                ctx.json(Map.of("status", "error", "message", "unknown action: " + incoming.action()));
                return;
            }
            Command command = new Command(incoming.key(), action, incoming.value());

            CommandResult result;
            try {
                result = raftNode.submitCommand(command).get(COMMAND_TIMEOUT_SECONDS, TimeUnit.SECONDS);
            } catch (TimeoutException e) {
                ctx.status(504);
                ctx.json(Map.of("status", "timeout"));
                return;
            } catch (ExecutionException e) {
                throw new RuntimeException(e.getCause());
            }

            switch (result.status()) {
                case COMMITTED -> ctx.json(Map.of(
                        "status", "committed",
                        "index", result.index(),
                        "key", result.key(),
                        "value", result.value()));
                case FAILED -> {
                    ctx.status(400);
                    ctx.json(Map.of(
                            "status", "error",
                            "index", result.index(),
                            "message", result.message()));
                }
                case NOT_LEADER -> {
                    ctx.status(409);
                    ctx.json(notLeaderResponse(raftNode.currentState().leaderId()));
                }
            }
        });

        app.exception(Exception.class, (e, ctx) -> {
            ctx.status(500);
            ctx.json(Map.of("status", "error", "message", String.valueOf(e.getMessage())));
        });
    }

    private List<Map<String, Object>> describeLog() {
        List<LogEntry> entries = logStore.getAll();
        List<Map<String, Object>> described = new ArrayList<>(entries.size());
        for (int i = 0; i < entries.size(); i++) {
            LogEntry entry = entries.get(i);
            Map<String, Object> item = new LinkedHashMap<>();
            item.put("index", i + 1);
            item.put("term", entry.term());
            item.put("command", entry.command());
            described.add(item);
        }
        return described;
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
