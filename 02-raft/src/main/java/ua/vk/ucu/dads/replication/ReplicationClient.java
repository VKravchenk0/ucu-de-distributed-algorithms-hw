package ua.vk.ucu.dads.replication;

import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import ua.vk.ucu.dads.config.NodeConfig;
import ua.vk.ucu.dads.grpc.RequestAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.RequestVoteRPC;
import ua.vk.ucu.dads.grpc.ReplicationServiceGrpc;
import ua.vk.ucu.dads.grpc.ResponseAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.ResponseVoteRPC;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.TimeUnit;

public class ReplicationClient implements PeerRpcClient {
    private static final long VOTE_DEADLINE_MS = 100;
    private static final long APPEND_ENTRIES_DEADLINE_MS = 250;

    private final Map<Integer, ManagedChannel> channels = new ConcurrentHashMap<>();
    private final Map<Integer, ReplicationServiceGrpc.ReplicationServiceBlockingStub> stubs = new ConcurrentHashMap<>();

    public ReplicationClient(Map<Integer, NodeConfig.PeerInfo> peers) {
        peers.forEach((id, peer) -> {
            ManagedChannel channel = ManagedChannelBuilder.forTarget(peer.grpcAddress()).usePlaintext().build();
            channels.put(id, channel);
            stubs.put(id, ReplicationServiceGrpc.newBlockingStub(channel));
        });
    }

    @Override
    public ResponseVoteRPC requestVote(int peerId, RequestVoteRPC request) {
        return stub(peerId, VOTE_DEADLINE_MS).requestVote(request);
    }

    @Override
    public ResponseAppendEntriesRPC appendEntries(int peerId, RequestAppendEntriesRPC request) {
        return stub(peerId, APPEND_ENTRIES_DEADLINE_MS).appendEntries(request);
    }

    private ReplicationServiceGrpc.ReplicationServiceBlockingStub stub(int peerId, long deadlineMs) {
        ReplicationServiceGrpc.ReplicationServiceBlockingStub stub = stubs.get(peerId);
        if (stub == null) {
            throw new IllegalArgumentException("Unknown peer id: " + peerId);
        }
        return stub.withDeadlineAfter(deadlineMs, TimeUnit.MILLISECONDS);
    }

    public void shutdown() {
        channels.values().forEach(ManagedChannel::shutdownNow);
    }
}
