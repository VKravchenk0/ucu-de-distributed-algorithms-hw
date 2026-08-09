package ua.vk.ucu.dads.raft.log;

import org.junit.jupiter.api.Test;

import ua.vk.ucu.dads.raft.log.LogEntry;
import ua.vk.ucu.dads.raft.log.LogStore;
import ua.vk.ucu.dads.statemachine.Command;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class LogStoreTest {

    private static LogEntry entry(int term, String key) {
        return new LogEntry(term, new Command(key, Command.Action.SET, 1));
    }

    @Test
    void startsEmpty() {
        LogStore store = new LogStore();

        assertTrue(store.getAll().isEmpty());
        assertEquals(0, store.lastIndex());
        assertEquals(0, store.lastTerm());
    }

    @Test
    void appendReturnsOneBasedIndexAndPreservesOrder() {
        LogStore store = new LogStore();

        assertEquals(1, store.append(entry(1, "a")));
        assertEquals(2, store.append(entry(1, "b")));

        assertEquals(List.of(entry(1, "a"), entry(1, "b")), store.getAll());
        assertEquals(2, store.lastIndex());
        assertEquals(1, store.lastTerm());
    }

    @Test
    void getReturnsEntryAtOneBasedIndex() {
        LogStore store = new LogStore();
        store.append(entry(1, "a"));
        store.append(entry(2, "b"));

        assertEquals(entry(1, "a"), store.get(1));
        assertEquals(entry(2, "b"), store.get(2));
    }

    @Test
    void getOutOfRangeThrows() {
        LogStore store = new LogStore();
        store.append(entry(1, "a"));

        org.junit.jupiter.api.Assertions.assertThrows(IndexOutOfBoundsException.class, () -> store.get(0));
        org.junit.jupiter.api.Assertions.assertThrows(IndexOutOfBoundsException.class, () -> store.get(2));
    }

    @Test
    void entriesFromReturnsSuffixAndEmptyPastTheEnd() {
        LogStore store = new LogStore();
        store.append(entry(1, "a"));
        store.append(entry(1, "b"));
        store.append(entry(2, "c"));

        assertEquals(List.of(entry(1, "b"), entry(2, "c")), store.entriesFrom(2));
        assertEquals(List.of(), store.entriesFrom(4));
    }

    @Test
    void truncateFromDeletesTailInclusive() {
        LogStore store = new LogStore();
        store.append(entry(1, "a"));
        store.append(entry(1, "b"));
        store.append(entry(2, "c"));

        store.truncateFrom(2);

        assertEquals(List.of(entry(1, "a")), store.getAll());
        assertEquals(1, store.lastIndex());
        assertEquals(1, store.lastTerm());
    }
}
