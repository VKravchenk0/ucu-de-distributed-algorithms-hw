package ua.vk.ucu.dads.log;

import ua.vk.ucu.dads.statemachine.Command;

/** A single Raft log entry: the term in which it was received by the leader, and its command. */
public record LogEntry(int term, Command command) {
}
