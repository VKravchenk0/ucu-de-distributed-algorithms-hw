package ua.vk.ucu.dads;

import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

public class LogStore {
    private final List<LogEntry> entries = new CopyOnWriteArrayList<>();

    public void append(LogEntry entry) {
        entries.add(entry);
    }

    public List<LogEntry> getAll() {
        return entries;
    }
}
