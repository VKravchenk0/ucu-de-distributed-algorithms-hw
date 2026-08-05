package ua.vk.ucu.dads.raft;

import ua.vk.ucu.dads.grpc.RequestAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.RequestVoteRPC;
import ua.vk.ucu.dads.grpc.ResponseAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.ResponseVoteRPC;
import ua.vk.ucu.dads.replication.PeerRpcClient;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Routes RPCs directly between in-memory {@link RaftNode} instances (calling their handler
 * methods in-process instead of going over gRPC), so multi-node Raft behavior can be exercised
 * in fast, deterministic unit tests. All nodes in a test share one instance, registering
 * themselves after construction and before {@code start()}.
 */
class InProcessNetwork implements PeerRpcClient {
    private final Map<Integer, RaftNode> nodes = new ConcurrentHashMap<>();

    void register(int nodeId, RaftNode node) {
        nodes.put(nodeId, node);
    }

    @Override
    public ResponseVoteRPC requestVote(int peerId, RequestVoteRPC request) {
        return target(peerId).handleRequestVote(request);
    }

    @Override
    public ResponseAppendEntriesRPC appendEntries(int peerId, RequestAppendEntriesRPC request) {
        return target(peerId).handleAppendEntries(request);
    }

    private RaftNode target(int peerId) {
        RaftNode node = nodes.get(peerId);
        if (node == null) {
            throw new IllegalStateException("no node registered for id " + peerId);
        }
        return node;
    }
}
