package ua.vk.ucu.dads.replication;

import io.grpc.stub.StreamObserver;
import ua.vk.ucu.dads.log.LogEntry;
import ua.vk.ucu.dads.log.LogStore;
import ua.vk.ucu.dads.grpc.AppendMessageRequest;
import ua.vk.ucu.dads.grpc.AppendMessageResponse;
import ua.vk.ucu.dads.grpc.ReplicationServiceGrpc;

public class ReplicationServer extends ReplicationServiceGrpc.ReplicationServiceImplBase {
    private final LogStore logStore;

    public ReplicationServer(LogStore logStore) {
        this.logStore = logStore;
    }

    @Override
    public void appendMessage(AppendMessageRequest request, StreamObserver<AppendMessageResponse> responseObserver) {
        logStore.append(new LogEntry(request.getMessage()));
        responseObserver.onNext(AppendMessageResponse.newBuilder().setSuccess(true).build());
        responseObserver.onCompleted();
    }
}
