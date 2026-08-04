package ua.vk.ucu.dads.replication;

import io.grpc.stub.StreamObserver;
import ua.vk.ucu.dads.grpc.RequestAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.RequestVoteRPC;
import ua.vk.ucu.dads.grpc.ReplicationServiceGrpc;
import ua.vk.ucu.dads.grpc.ResponseAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.ResponseVoteRPC;
import ua.vk.ucu.dads.raft.RaftNode;

public class ReplicationServer extends ReplicationServiceGrpc.ReplicationServiceImplBase {
    private final RaftNode raftNode;

    public ReplicationServer(RaftNode raftNode) {
        this.raftNode = raftNode;
    }

    @Override
    public void requestVote(RequestVoteRPC request, StreamObserver<ResponseVoteRPC> responseObserver) {
        responseObserver.onNext(raftNode.handleRequestVote(request));
        responseObserver.onCompleted();
    }

    @Override
    public void appendEntries(RequestAppendEntriesRPC request, StreamObserver<ResponseAppendEntriesRPC> responseObserver) {
        responseObserver.onNext(raftNode.handleAppendEntries(request));
        responseObserver.onCompleted();
    }
}
