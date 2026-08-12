package ua.vk.ucu.dads.raft.log;

import ua.vk.ucu.dads.statemachine.Command;

public record LogEntry(int term, Command command) {
}
