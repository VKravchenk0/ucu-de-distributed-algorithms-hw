package ua.vk.ucu.dads.statemachine;

import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.Map;

/**
 * A simple in-memory key/value state machine. {@link #apply} is invoked exclusively from the
 * {@code RaftNode} event loop as committed entries are applied, in log order, so mutation itself
 * needs no locking; methods are synchronized only so {@link #snapshot()} - called from HTTP
 * handler threads for the debug endpoint - sees a consistent view.
 */
public class StateMachine {
    private final Map<String, Integer> values = new HashMap<>();

    public synchronized int apply(Command command) {
        return switch (command.action()) {
            case SET -> {
                values.put(command.key(), command.value());
                yield command.value();
            }
            case ADD -> update(command.key(), command.value());
            case SUBTRACT -> update(command.key(), -command.value());
        };
    }

    private int update(String key, int delta) {
        Integer current = values.get(key);
        if (current == null) {
            throw new UnknownKeyException(key);
        }
        int updated = current + delta;
        values.put(key, updated);
        return updated;
    }

    public synchronized Map<String, Integer> snapshot() {
        return new LinkedHashMap<>(values);
    }
}
