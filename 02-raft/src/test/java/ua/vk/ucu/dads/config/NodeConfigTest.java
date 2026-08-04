package ua.vk.ucu.dads.config;

import org.junit.jupiter.api.Test;

import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class NodeConfigTest {

    @Test
    void defaultsWhenEnvEmpty() {
        NodeConfig config = NodeConfig.from(Map.of());

        assertFalse(config.isMaster());
        assertTrue(config.secondaryAddresses().isEmpty());
        assertEquals("", config.masterUrl());
    }

    @Test
    void parsesMasterWithSecondariesTrimmingAndDroppingEmpties() {
        NodeConfig config = NodeConfig.from(Map.of(
                "IS_MASTER", "true",
                "SECONDARY_ADDRESSES", "host1:6001, host2:6001 ,,"
        ));

        assertTrue(config.isMaster());
        assertEquals(List.of("host1:6001", "host2:6001"), config.secondaryAddresses());
    }

    @Test
    void parsesSecondaryWithMasterUrl() {
        NodeConfig config = NodeConfig.from(Map.of(
                "IS_MASTER", "false",
                "MASTER_URL", "http://master:7000"
        ));

        assertFalse(config.isMaster());
        assertEquals("http://master:7000", config.masterUrl());
    }
}
