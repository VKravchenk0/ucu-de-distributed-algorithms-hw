package ua.vk.ucu.dads.statemachine;

/**
 * A client command applied to the {@link StateMachine}: {@code action} on {@code key} with
 * {@code value}. {@code SET} creates or overwrites the key; {@code ADD}/{@code SUBTRACT}
 * require the key to already exist.
 */
public record Command(String key, Action action, int value) {
    public enum Action {
        SET,
        ADD,
        SUBTRACT
    }
}
