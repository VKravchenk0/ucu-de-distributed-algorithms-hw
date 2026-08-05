package ua.vk.ucu.dads.raft;

/** Outcome of a client command submitted via {@link RaftNode#submitCommand}. */
public record CommandResult(Status status, int index, String key, Integer value, String message) {
    public enum Status {
        COMMITTED,
        FAILED,
        NOT_LEADER
    }

    public static CommandResult committed(int index, String key, int value) {
        return new CommandResult(Status.COMMITTED, index, key, value, null);
    }

    public static CommandResult failed(int index, String key, String message) {
        return new CommandResult(Status.FAILED, index, key, null, message);
    }

    public static CommandResult notLeader() {
        return new CommandResult(Status.NOT_LEADER, 0, null, null, null);
    }
}
