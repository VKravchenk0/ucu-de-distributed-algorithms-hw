package ua.vk.ucu.dads.statemachine;

import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.Map;

/**
 * A simple in-memory key/value state machine.
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
