package ua.vk.ucu.dads.raft;

public record ServerState(int nodeId, int currentTerm, ServerStatus status, Integer leaderId) {
}
