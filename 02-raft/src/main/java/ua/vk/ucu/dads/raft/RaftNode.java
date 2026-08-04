package ua.vk.ucu.dads.raft;

import ua.vk.ucu.dads.config.NodeConfig;
import ua.vk.ucu.dads.grpc.RequestAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.RequestVoteRPC;
import ua.vk.ucu.dads.grpc.ResponseAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.ResponseVoteRPC;
import ua.vk.ucu.dads.log.LogEntry;
import ua.vk.ucu.dads.log.LogStore;
import ua.vk.ucu.dads.replication.PeerRpcClient;

import java.util.Random;
import java.util.concurrent.Callable;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

/**
 * Owns all Raft election state (currentTerm, votedFor, status, currentLeaderId). All mutations
 * happen on a single-threaded "event loop" executor, so no locking is needed - the event loop
 * is the single writer by construction. Inbound RPC handlers submit work to the event loop and
 * block for the result; outbound RPC fan-out (RequestVote during an election, AppendEntries
 * heartbeats) runs on a separate pool so a slow/dead peer never stalls the event loop, with
 * result processing resubmitted back onto the event loop.
 */
public class RaftNode {
    private static final int ELECTION_TIMEOUT_MIN_MS = 150;
    private static final int ELECTION_TIMEOUT_MAX_MS = 300;
    private static final long HEARTBEAT_INTERVAL_MS = 50;
    private static final long RPC_HANDLER_TIMEOUT_MS = 300;

    private final int nodeId;
    private final NodeConfig config;
    private final LogStore logStore;
    private final PeerRpcClient rpcClient;
    private final Random random = new Random();

    private final ScheduledExecutorService eventLoop = Executors.newSingleThreadScheduledExecutor();
    private final ExecutorService rpcExecutor;

    // Mutated exclusively on the eventLoop thread.
    private int currentTerm = 0;
    private Integer votedFor = null;
    private ServerStatus status = ServerStatus.FOLLOWER;
    private Integer currentLeaderId = null;
    private int votesReceived = 0;
    private ScheduledFuture<?> electionTimeoutFuture;
    private ScheduledFuture<?> heartbeatFuture;

    private volatile ServerState snapshot;

    public RaftNode(NodeConfig config, LogStore logStore, PeerRpcClient rpcClient) {
        this.nodeId = config.nodeId();
        this.config = config;
        this.logStore = logStore;
        this.rpcClient = rpcClient;
        this.rpcExecutor = Executors.newFixedThreadPool(Math.max(1, config.peers().size()));
        this.snapshot = new ServerState(nodeId, currentTerm, status, currentLeaderId);
    }

    public void start() {
        eventLoop.execute(this::resetElectionTimer);
    }

    public void shutdown() {
        eventLoop.shutdownNow();
        rpcExecutor.shutdownNow();
    }

    public ServerState currentState() {
        return snapshot;
    }

    public ResponseVoteRPC handleRequestVote(RequestVoteRPC request) {
        return runOnEventLoop(() -> handleRequestVoteInternal(request));
    }

    public ResponseAppendEntriesRPC handleAppendEntries(RequestAppendEntriesRPC request) {
        return runOnEventLoop(() -> handleAppendEntriesInternal(request));
    }

    private <T> T runOnEventLoop(Callable<T> task) {
        try {
            return eventLoop.submit(task).get(RPC_HANDLER_TIMEOUT_MS, TimeUnit.MILLISECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new RuntimeException(e);
        } catch (ExecutionException | TimeoutException e) {
            throw new RuntimeException(e);
        }
    }

    // --- Event-loop-thread-only methods below ---

    private void resetElectionTimer() {
        if (electionTimeoutFuture != null) {
            electionTimeoutFuture.cancel(false);
        }
        long delay = ELECTION_TIMEOUT_MIN_MS + random.nextInt(ELECTION_TIMEOUT_MAX_MS - ELECTION_TIMEOUT_MIN_MS + 1);
        electionTimeoutFuture = eventLoop.schedule(this::onElectionTimeout, delay, TimeUnit.MILLISECONDS);
    }

    private void onElectionTimeout() {
        startElectionRound();
    }

    private void startElectionRound() {
        currentTerm++;
        status = ServerStatus.CANDIDATE;
        votedFor = nodeId;
        votesReceived = 1;
        currentLeaderId = null;
        publishSnapshot();
        resetElectionTimer();

        if (votesReceived >= config.majority()) {
            becomeLeader();
            return;
        }

        int roundTerm = currentTerm;
        int lastLogIndex = logStore.getAll().size();
        for (int peerId : config.peers().keySet()) {
            rpcExecutor.submit(() -> {
                try {
                    RequestVoteRPC request = RequestVoteRPC.newBuilder()
                            .setTerm(roundTerm)
                            .setCandidateId(nodeId)
                            .setLastLogIndex(lastLogIndex)
                            .setLastLogTerm(0) // placeholder: LogEntry carries no term yet (out of scope this step)
                            .build();
                    ResponseVoteRPC response = rpcClient.requestVote(peerId, request);
                    eventLoop.execute(() -> onVoteResponse(roundTerm, response));
                } catch (Exception e) {
                    // peer unreachable this round - ignore, next election timeout retries
                }
            });
        }
    }

    private void onVoteResponse(int roundTerm, ResponseVoteRPC response) {
        if (response.getTerm() > currentTerm) {
            becomeFollower(response.getTerm());
            return;
        }
        if (status != ServerStatus.CANDIDATE || currentTerm != roundTerm) {
            return; // stale response from a round we've since moved past
        }
        if (response.getVoteGranted()) {
            votesReceived++;
            if (votesReceived >= config.majority()) {
                becomeLeader();
            }
        }
    }

    private void becomeLeader() {
        status = ServerStatus.LEADER;
        currentLeaderId = nodeId;
        if (electionTimeoutFuture != null) {
            electionTimeoutFuture.cancel(false);
        }
        publishSnapshot();
        sendHeartbeatRound();
        heartbeatFuture = eventLoop.scheduleAtFixedRate(
                this::sendHeartbeatRound, HEARTBEAT_INTERVAL_MS, HEARTBEAT_INTERVAL_MS, TimeUnit.MILLISECONDS);
    }

    private void sendHeartbeatRound() {
        if (status != ServerStatus.LEADER) {
            return;
        }
        int roundTerm = currentTerm;
        for (int peerId : config.peers().keySet()) {
            rpcExecutor.submit(() -> {
                try {
                    RequestAppendEntriesRPC request = RequestAppendEntriesRPC.newBuilder()
                            .setTerm(roundTerm)
                            .setLeaderId(nodeId)
                            .setPrevLogIndex(0)
                            .setPrevLogTerm(0)
                            .setLeaderCommit(0)
                            .build();
                    ResponseAppendEntriesRPC response = rpcClient.appendEntries(peerId, request);
                    eventLoop.execute(() -> onHeartbeatResponse(response));
                } catch (Exception e) {
                    // peer unreachable this round - ignore, next heartbeat tick retries
                }
            });
        }
    }

    private void onHeartbeatResponse(ResponseAppendEntriesRPC response) {
        if (response.getTerm() > currentTerm) {
            becomeFollower(response.getTerm());
        }
    }

    private void becomeFollower(int seenTerm) {
        if (seenTerm > currentTerm) {
            currentTerm = seenTerm;
            votedFor = null;
        }
        status = ServerStatus.FOLLOWER;
        if (heartbeatFuture != null) {
            heartbeatFuture.cancel(false);
            heartbeatFuture = null;
        }
        publishSnapshot();
        resetElectionTimer();
    }

    private ResponseVoteRPC handleRequestVoteInternal(RequestVoteRPC request) {
        if (request.getTerm() < currentTerm) {
            return ResponseVoteRPC.newBuilder().setTerm(currentTerm).setVoteGranted(false).build();
        }
        if (request.getTerm() > currentTerm) {
            becomeFollower(request.getTerm());
        }
        boolean granted = votedFor == null || votedFor == request.getCandidateId();
        if (granted) {
            votedFor = request.getCandidateId();
            resetElectionTimer();
        }
        publishSnapshot();
        return ResponseVoteRPC.newBuilder().setTerm(currentTerm).setVoteGranted(granted).build();
    }

    private ResponseAppendEntriesRPC handleAppendEntriesInternal(RequestAppendEntriesRPC request) {
        if (request.getTerm() < currentTerm) {
            return ResponseAppendEntriesRPC.newBuilder().setTerm(currentTerm).setSuccess(false).build();
        }
        if (request.getTerm() > currentTerm || status != ServerStatus.FOLLOWER) {
            becomeFollower(request.getTerm());
        }
        currentLeaderId = request.getLeaderId();
        resetElectionTimer();
        for (RequestAppendEntriesRPC.LogEntry entry : request.getEntriesList()) {
            logStore.append(new LogEntry(entry.getCommand()));
        }
        publishSnapshot();
        return ResponseAppendEntriesRPC.newBuilder().setTerm(currentTerm).setSuccess(true).build();
    }

    private void publishSnapshot() {
        snapshot = new ServerState(nodeId, currentTerm, status, currentLeaderId);
    }
}
