package ua.vk.ucu.dads.log;

import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class LogStoreTest {

    @Test
    void startsEmpty() {
        LogStore store = new LogStore();

        assertTrue(store.getAll().isEmpty());
    }

    @Test
    void appendThenGetAllReturnsInOrder() {
        LogStore store = new LogStore();

        store.append(new LogEntry("first"));
        store.append(new LogEntry("second"));

        assertEquals(List.of(new LogEntry("first"), new LogEntry("second")), store.getAll());
    }
}
