package ua.vk.ucu.dads.statemachine;

import org.junit.jupiter.api.Test;

import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class StateMachineTest {

    @Test
    void setCreatesOrOverwritesKey() {
        StateMachine sm = new StateMachine();

        assertEquals(5, sm.apply(new Command("x", Command.Action.SET, 5)));
        assertEquals(Map.of("x", 5), sm.snapshot());

        assertEquals(9, sm.apply(new Command("x", Command.Action.SET, 9)));
        assertEquals(Map.of("x", 9), sm.snapshot());
    }

    @Test
    void addAndSubtractUpdateExistingKey() {
        StateMachine sm = new StateMachine();
        sm.apply(new Command("x", Command.Action.SET, 5));

        assertEquals(7, sm.apply(new Command("x", Command.Action.ADD, 2)));
        assertEquals(4, sm.apply(new Command("x", Command.Action.SUBTRACT, 3)));
        assertEquals(Map.of("x", 4), sm.snapshot());
    }

    @Test
    void addOnMissingKeyThrowsAndLeavesStateUnchanged() {
        StateMachine sm = new StateMachine();

        assertThrows(UnknownKeyException.class, () -> sm.apply(new Command("missing", Command.Action.ADD, 1)));
        assertEquals(Map.of(), sm.snapshot());
    }

    @Test
    void subtractOnMissingKeyThrowsAndLeavesStateUnchanged() {
        StateMachine sm = new StateMachine();

        assertThrows(UnknownKeyException.class, () -> sm.apply(new Command("missing", Command.Action.SUBTRACT, 1)));
        assertEquals(Map.of(), sm.snapshot());
    }
}
