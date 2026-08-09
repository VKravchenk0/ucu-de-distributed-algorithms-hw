package ua.vk.ucu.dads.raft;

import ua.vk.ucu.dads.config.NodeConfig;
import ua.vk.ucu.dads.grpc.RequestAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.RequestVoteRPC;
import ua.vk.ucu.dads.grpc.ResponseAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.ResponseVoteRPC;
import ua.vk.ucu.dads.raft.log.LogEntry;
import ua.vk.ucu.dads.raft.log.LogStore;
import ua.vk.ucu.dads.replication.PeerRpcClient;
import ua.vk.ucu.dads.replication.ProtoMapper;
import ua.vk.ucu.dads.statemachine.Command;
import ua.vk.ucu.dads.statemachine.StateMachine;
import ua.vk.ucu.dads.statemachine.UnknownKeyException;

import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Random;
import java.util.Set;
import java.util.concurrent.Callable;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

/**
 * Owns all Raft state (currentTerm, votedFor, status, currentLeaderId, log, commitIndex,
 * lastApplied, and - while leader - nextIndex/matchIndex per peer). All mutations happen on a
 * single-threaded "event loop" executor, so no locking is needed - the event loop is the single
 * writer by construction. Inbound RPC handlers submit work to the event loop and block for the
 * result; outbound RPC fan-out (RequestVote during an election, AppendEntries for heartbeats and
 * replication) runs on a separate pool of virtual threads so a slow/dead peer never stalls the
 * event loop, with result processing resubmitted back onto the event loop.
 */
public class RaftNode {
    private static final int ELECTION_TIMEOUT_MIN_MS = 150;
    private static final int ELECTION_TIMEOUT_MAX_MS = 300;
    private static final long HEARTBEAT_INTERVAL_MS = 50;
    private static final long RPC_HANDLER_TIMEOUT_MS = 500;

    private final int nodeId;
    private final NodeConfig config;
    private final LogStore logStore;
    private final PeerRpcClient rpcClient;
    private final StateMachine stateMachine;
    private final Random random = new Random();

    private final ScheduledExecutorService eventLoop = Executors.newSingleThreadScheduledExecutor();
    private final ExecutorService rpcExecutor = Executors.newVirtualThreadPerTaskExecutor();

    // Mutated exclusively on the eventLoop thread.
    private int currentTerm = 0;
    private Integer votedFor = null;
    private ServerStatus status = ServerStatus.FOLLOWER;
    private Integer currentLeaderId = null;
    private int votesReceived = 0;
    private ScheduledFuture<?> electionTimeoutFuture;
    private ScheduledFuture<?> heartbeatFuture;

    private int commitIndex = 0;
    private int lastApplied = 0;
    // Leader-only volatile state, reinitialized in becomeLeader().
    private final Map<Integer, Integer> nextIndex = new HashMap<>();
    private final Map<Integer, Integer> matchIndex = new HashMap<>();
    // Peers with an AppendEntries RPC currently in flight - prevents piling up requests behind a slow peer.
    private final Set<Integer> inFlight = new HashSet<>();
    // Client futures waiting on the entry at a given log index being committed and applied.
    private final Map<Integer, CompletableFuture<CommandResult>> pendingClients = new HashMap<>();

    private volatile ServerState snapshot;

    public RaftNode(NodeConfig config, LogStore logStore, PeerRpcClient rpcClient, StateMachine stateMachine) {
        this.nodeId = config.nodeId();
        this.config = config;
        this.logStore = logStore;
        this.rpcClient = rpcClient;
        this.stateMachine = stateMachine;
        this.snapshot = new ServerState(nodeId, currentTerm, status, currentLeaderId, commitIndex, lastApplied, logStore.lastIndex());
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

    /**
     * Submits a client command for replication. Completes once the resulting log entry has been
     * committed (replicated to a majority) and applied to the state machine, or immediately with
     * {@code NOT_LEADER} if this node isn't the leader.
     */
    public CompletableFuture<CommandResult> submitCommand(Command command) {
        CompletableFuture<CommandResult> future = new CompletableFuture<>();
        eventLoop.execute(() -> {
            if (status != ServerStatus.LEADER) {
                future.complete(CommandResult.notLeader());
                return;
            }
            int index = logStore.append(new LogEntry(currentTerm, command));
            pendingClients.put(index, future);
            replicateToAllPeers();
            advanceCommitIndex();
            applyCommitted();
        });
        return future;
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
        int lastLogIndex = logStore.lastIndex();
        int lastLogTerm = logStore.lastTerm();
        for (int peerId : config.peers().keySet()) {
            rpcExecutor.submit(() -> {
                try {
                    RequestVoteRPC request = RequestVoteRPC.newBuilder()
                            .setTerm(roundTerm)
                            .setCandidateId(nodeId)
                            .setLastLogIndex(lastLogIndex)
                            .setLastLogTerm(lastLogTerm)
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
        int next = logStore.lastIndex() + 1;
        nextIndex.clear();
        matchIndex.clear();
        inFlight.clear();
        for (int peerId : config.peers().keySet()) {
            nextIndex.put(peerId, next);
            matchIndex.put(peerId, 0);
        }
        publishSnapshot();
        replicateToAllPeers();
        heartbeatFuture = eventLoop.scheduleAtFixedRate(
                this::replicateToAllPeers, HEARTBEAT_INTERVAL_MS, HEARTBEAT_INTERVAL_MS, TimeUnit.MILLISECONDS);
    }

    /** Periodic tick (also invoked eagerly after appends): the "retry indefinitely" mechanism. */
    private void replicateToAllPeers() {
        if (status != ServerStatus.LEADER) {
            return;
        }
        for (int peerId : config.peers().keySet()) {
            replicateTo(peerId);
        }
    }

    private void replicateTo(int peerId) {
        if (status != ServerStatus.LEADER) {
            return;
        }
        if (!inFlight.add(peerId)) {
            return; // an AppendEntries RPC to this peer is already outstanding
        }
        int roundTerm = currentTerm;
        int next = nextIndex.getOrDefault(peerId, logStore.lastIndex() + 1);
        int prevLogIndex = next - 1;
        int prevLogTerm = prevLogIndex == 0 ? 0 : logStore.get(prevLogIndex).term();
        List<LogEntry> toSend = logStore.entriesFrom(next);
        int sentCount = toSend.size();
        int leaderCommit = commitIndex;

        RequestAppendEntriesRPC request = RequestAppendEntriesRPC.newBuilder()
                .setTerm(roundTerm)
                .setLeaderId(nodeId)
                .setPrevLogIndex(prevLogIndex)
                .setPrevLogTerm(prevLogTerm)
                .addAllEntries(ProtoMapper.toProto(toSend))
                .setLeaderCommit(leaderCommit)
                .build();

        rpcExecutor.submit(() -> {
            try {
                ResponseAppendEntriesRPC response = rpcClient.appendEntries(peerId, request);
                eventLoop.execute(() -> {
                    inFlight.remove(peerId);
                    onAppendEntriesResponse(peerId, roundTerm, prevLogIndex, sentCount, response);
                });
            } catch (Exception e) {
                // peer unreachable this round - ignore, next tick (or the eager retry below) retries
                eventLoop.execute(() -> inFlight.remove(peerId));
            }
        });
    }

    private void onAppendEntriesResponse(int peerId, int roundTerm, int prevLogIndex, int sentCount,
                                          ResponseAppendEntriesRPC response) {
        if (response.getTerm() > currentTerm) {
            becomeFollower(response.getTerm());
            return;
        }
        if (status != ServerStatus.LEADER || currentTerm != roundTerm) {
            return; // stale round - we've since stepped down or moved to a new term
        }
        if (response.getSuccess()) {
            int newMatch = prevLogIndex + sentCount;
            matchIndex.merge(peerId, newMatch, Math::max);
            nextIndex.put(peerId, matchIndex.get(peerId) + 1);
            advanceCommitIndex();
            applyCommitted();
            if (nextIndex.get(peerId) <= logStore.lastIndex()) {
                replicateTo(peerId); // more entries to stream - don't wait for the next heartbeat tick
            }
        } else {
            int next = Math.max(1, nextIndex.getOrDefault(peerId, prevLogIndex + 1) - 1);
            nextIndex.put(peerId, next);
            replicateTo(peerId); // retry immediately with a lower nextIndex
        }
    }

    /** Advances commitIndex to the highest index replicated on a majority, from the current term only (§5.4.2). */
    private void advanceCommitIndex() {
        if (status != ServerStatus.LEADER) {
            return;
        }
        for (int n = logStore.lastIndex(); n > commitIndex; n--) {
            if (logStore.get(n).term() != currentTerm) {
                break;
            }
            int replicatedCount = 1; // the leader itself
            for (int match : matchIndex.values()) {
                if (match >= n) {
                    replicatedCount++;
                }
            }
            if (replicatedCount >= config.majority()) {
                commitIndex = n;
                break;
            }
        }
    }

    /** Applies newly committed entries to the state machine, in order, and resolves any waiting client futures. */
    private void applyCommitted() {
        while (lastApplied < commitIndex) {
            lastApplied++;
            LogEntry entry = logStore.get(lastApplied);
            CompletableFuture<CommandResult> pending = pendingClients.remove(lastApplied);
            try {
                int value = stateMachine.apply(entry.command());
                if (pending != null) {
                    pending.complete(CommandResult.committed(lastApplied, entry.command().key(), value));
                }
            } catch (UnknownKeyException e) {
                if (pending != null) {
                    pending.complete(CommandResult.failed(lastApplied, entry.command().key(), e.getMessage()));
                }
            }
        }
        publishSnapshot();
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
        failPendingClients();
        publishSnapshot();
        resetElectionTimer();
    }

    /** A stepped-down (or never-was) leader must not leave HTTP clients hanging. */
    private void failPendingClients() {
        if (pendingClients.isEmpty()) {
            return;
        }
        pendingClients.values().forEach(future -> future.complete(CommandResult.notLeader()));
        pendingClients.clear();
    }

    private ResponseVoteRPC handleRequestVoteInternal(RequestVoteRPC request) {
        if (request.getTerm() < currentTerm) {
            return ResponseVoteRPC.newBuilder().setTerm(currentTerm).setVoteGranted(false).build();
        }
        if (request.getTerm() > currentTerm) {
            becomeFollower(request.getTerm());
        }
        boolean logUpToDate = isLogUpToDate(request.getLastLogTerm(), request.getLastLogIndex());
        boolean granted = (votedFor == null || votedFor == request.getCandidateId()) && logUpToDate;
        if (granted) {
            votedFor = request.getCandidateId();
            resetElectionTimer();
        }
        publishSnapshot();
        return ResponseVoteRPC.newBuilder().setTerm(currentTerm).setVoteGranted(granted).build();
    }

    /** Election restriction (§5.4.1): grant a vote only to a candidate whose log is at least as up to date as ours. */
    private boolean isLogUpToDate(int candidateLastLogTerm, int candidateLastLogIndex) {
        int ourLastTerm = logStore.lastTerm();
        int ourLastIndex = logStore.lastIndex();
        if (candidateLastLogTerm != ourLastTerm) {
            return candidateLastLogTerm > ourLastTerm;
        }
        return candidateLastLogIndex >= ourLastIndex;
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

        int prevLogIndex = request.getPrevLogIndex();
        int prevLogTerm = request.getPrevLogTerm();
        if (prevLogIndex > logStore.lastIndex()
                || (prevLogIndex > 0 && logStore.get(prevLogIndex).term() != prevLogTerm)) {
            publishSnapshot();
            return ResponseAppendEntriesRPC.newBuilder().setTerm(currentTerm).setSuccess(false).build();
        }

        int idx = prevLogIndex;
        for (ua.vk.ucu.dads.grpc.LogEntry protoEntry : request.getEntriesList()) {
            idx++;
            LogEntry entry = ProtoMapper.fromProto(protoEntry);
            if (idx <= logStore.lastIndex()) {
                if (logStore.get(idx).term() == entry.term()) {
                    continue; // already present - this may be a retransmit, don't touch the log
                }
                logStore.truncateFrom(idx); // conflict: delete this entry and everything after it
            }
            logStore.append(entry);
        }

        if (request.getLeaderCommit() > commitIndex) {
            commitIndex = Math.min(request.getLeaderCommit(), idx);
        }
        applyCommitted();
        return ResponseAppendEntriesRPC.newBuilder().setTerm(currentTerm).setSuccess(true).build();
    }

    private void publishSnapshot() {
        snapshot = new ServerState(nodeId, currentTerm, status, currentLeaderId, commitIndex, lastApplied, logStore.lastIndex());
    }
}
