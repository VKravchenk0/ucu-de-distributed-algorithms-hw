package ua.vk.ucu.dads.replication;

import ua.vk.ucu.dads.grpc.RequestAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.RequestVoteRPC;
import ua.vk.ucu.dads.grpc.ResponseAppendEntriesRPC;
import ua.vk.ucu.dads.grpc.ResponseVoteRPC;

/**
 * Outbound RPCs to a single peer, addressed by node id. Implementations are expected to throw
 * an unchecked exception (timeout, unreachable, etc.) on failure - callers treat any exception
 * as "peer unreachable this round, ignore, retry next timer tick".
 */
public interface PeerRpcClient {
    ResponseVoteRPC requestVote(int peerId, RequestVoteRPC request);

    ResponseAppendEntriesRPC appendEntries(int peerId, RequestAppendEntriesRPC request);
}
