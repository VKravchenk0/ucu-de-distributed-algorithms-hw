package ua.vk.ucu.dads.replication;

import io.grpc.ManagedChannel;
import io.grpc.ManagedChannelBuilder;
import ua.vk.ucu.dads.grpc.RequestAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.ReplicationServiceGrpc;

import java.util.List;
import java.util.stream.Collectors;

public class ReplicationClient {
    private final List<ManagedChannel> channels;
    private final List<ReplicationServiceGrpc.ReplicationServiceBlockingStub> stubs;

    public ReplicationClient(List<String> secondaryAddresses) {
        this.channels = secondaryAddresses.stream()
                .map(addr -> ManagedChannelBuilder.forTarget(addr).usePlaintext().build())
                .collect(Collectors.toList());
        this.stubs = channels.stream()
                .map(ReplicationServiceGrpc::newBlockingStub)
                .collect(Collectors.toList());
    }

    public void replicateToAllSecondaries(String message) {
        RequestAppendEntriesRPC.LogEntry entry = RequestAppendEntriesRPC.LogEntry.newBuilder()
                .setCommand(message)
                .build();
        RequestAppendEntriesRPC request = RequestAppendEntriesRPC.newBuilder()
                .addEntries(entry)
                .build();
        for (var stub : stubs) {
            stub.appendEntries(request);
        }
    }

    public void shutdown() {
        channels.forEach(ManagedChannel::shutdownNow);
    }
}
