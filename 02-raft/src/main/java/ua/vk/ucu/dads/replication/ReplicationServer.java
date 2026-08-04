package ua.vk.ucu.dads.replication;

import io.grpc.stub.StreamObserver;
import ua.vk.ucu.dads.log.LogEntry;
import ua.vk.ucu.dads.log.LogStore;
import ua.vk.ucu.dads.grpc.RequestAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.ResponseAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.ReplicationServiceGrpc;

public class ReplicationServer extends ReplicationServiceGrpc.ReplicationServiceImplBase {
    private final LogStore logStore;

    public ReplicationServer(LogStore logStore) {
        this.logStore = logStore;
    }

    @Override
    public void appendEntries(RequestAppendEntriesRPC request, StreamObserver<ResponseAppendEntriesRPC> responseObserver) {
        for (RequestAppendEntriesRPC.LogEntry entry : request.getEntriesList()) {
            logStore.append(new LogEntry(entry.getCommand()));
        }
        responseObserver.onNext(ResponseAppendEntriesRPC.newBuilder().setTerm(request.getTerm()).setSuccess(true).build());
        responseObserver.onCompleted();
    }
}
