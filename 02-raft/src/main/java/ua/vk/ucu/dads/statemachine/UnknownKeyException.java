package ua.vk.ucu.dads.statemachine;

/** Thrown when a command targets a key that does not exist yet (e.g. ADD/SUBTRACT before SET). */
public class UnknownKeyException extends RuntimeException {
    public UnknownKeyException(String key) {
        super("unknown key: " + key);
    }
}
