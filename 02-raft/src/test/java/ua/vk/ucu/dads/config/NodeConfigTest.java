package ua.vk.ucu.dads.config;

import org.junit.jupiter.api.Test;

import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class NodeConfigTest {

    @Test
    void requiresNodeId() {
        assertThrows(IllegalArgumentException.class, () -> NodeConfig.from(Map.of()));
    }

    @Test
    void parsesNodeIdWithNoPeers() {
        NodeConfig config = NodeConfig.from(Map.of("NODE_ID", "1"));

        assertEquals(1, config.nodeId());
        assertTrue(config.peers().isEmpty());
        assertEquals(1, config.clusterSize());
        assertEquals(1, config.majority());
    }

    @Test
    void parsesPeersTrimmingAndDroppingEmptyEntries() {
        NodeConfig config = NodeConfig.from(Map.of(
                "NODE_ID", "1",
                "PEERS", " 2=host2:6001:7000 , 3=host3:6001:7000 ,,"
        ));

        assertEquals(1, config.nodeId());
        assertEquals(2, config.peers().size());
        assertEquals(new NodeConfig.PeerInfo("host2:6001", "http://host2:7000"), config.peers().get(2));
        assertEquals(new NodeConfig.PeerInfo("host3:6001", "http://host3:7000"), config.peers().get(3));
        assertEquals(3, config.clusterSize());
        assertEquals(2, config.majority());
    }

    @Test
    void rejectsMalformedPeerEntry() {
        assertThrows(IllegalArgumentException.class, () -> NodeConfig.from(Map.of(
                "NODE_ID", "1",
                "PEERS", "2=host2:6001"
        )));
    }
}
