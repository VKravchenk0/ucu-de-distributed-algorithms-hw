package ua.vk.ucu.dads.log;

import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

/**
 * The Raft log. Entries are addressed with the paper's 1-based indexing: the first appended
 * entry is at index 1. Only ever mutated from the {@code RaftNode} event loop thread.
 */
public class LogStore {
    private final List<LogEntry> entries = new CopyOnWriteArrayList<>();

    /** Appends the entry and returns its (1-based) index. */
    public int append(LogEntry entry) {
        entries.add(entry);
        return entries.size();
    }

    /** Returns the entry at the given 1-based index. */
    public LogEntry get(int index) {
        if (index < 1 || index > entries.size()) {
            throw new IndexOutOfBoundsException("no log entry at index " + index);
        }
        return entries.get(index - 1);
    }

    /** Index of the last entry, or 0 if the log is empty. */
    public int lastIndex() {
        return entries.size();
    }

    /** Term of the last entry, or 0 if the log is empty. */
    public int lastTerm() {
        return entries.isEmpty() ? 0 : entries.get(entries.size() - 1).term();
    }

    /** All entries from {@code index} (1-based, inclusive) to the end; empty if past the end. */
    public List<LogEntry> entriesFrom(int index) {
        if (index < 1) {
            index = 1;
        }
        if (index > entries.size()) {
            return List.of();
        }
        return List.copyOf(entries.subList(index - 1, entries.size()));
    }

    /** Deletes all entries from {@code index} (1-based, inclusive) to the end. */
    public void truncateFrom(int index) {
        while (entries.size() >= index) {
            entries.remove(entries.size() - 1);
        }
    }

    public List<LogEntry> getAll() {
        return entries;
    }
}
