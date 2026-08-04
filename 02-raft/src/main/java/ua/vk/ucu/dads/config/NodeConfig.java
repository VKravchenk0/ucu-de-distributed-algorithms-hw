package ua.vk.ucu.dads.config;

import java.util.HashMap;
import java.util.Map;

public record NodeConfig(int nodeId, Map<Integer, PeerInfo> peers) {

    public record PeerInfo(String grpcAddress, String httpAddress) {
    }

    public static NodeConfig fromEnv() {
        return from(System.getenv());
    }

    public static NodeConfig from(Map<String, String> env) {
        String rawNodeId = env.get("NODE_ID");
        if (rawNodeId == null || rawNodeId.isBlank()) {
            throw new IllegalArgumentException("NODE_ID is required");
        }
        int nodeId = Integer.parseInt(rawNodeId.trim());
        Map<Integer, PeerInfo> peers = parsePeers(env.get("PEERS"));
        return new NodeConfig(nodeId, peers);
    }

    public int clusterSize() {
        return peers.size() + 1;
    }

    public int majority() {
        return clusterSize() / 2 + 1;
    }

    private static Map<Integer, PeerInfo> parsePeers(String raw) {
        Map<Integer, PeerInfo> peers = new HashMap<>();
        if (raw == null || raw.isBlank()) {
            return peers;
        }
        for (String entry : raw.split(",")) {
            String trimmed = entry.trim();
            if (trimmed.isEmpty()) {
                continue;
            }
            String[] idAndAddress = trimmed.split("=", 2);
            if (idAndAddress.length != 2) {
                throw new IllegalArgumentException("Invalid PEERS entry: " + trimmed);
            }
            int id = Integer.parseInt(idAndAddress[0].trim());
            String[] parts = idAndAddress[1].trim().split(":");
            if (parts.length != 3) {
                throw new IllegalArgumentException("Invalid PEERS address (expected host:grpcPort:httpPort): " + trimmed);
            }
            String host = parts[0];
            String grpcAddress = host + ":" + parts[1];
            String httpAddress = "http://" + host + ":" + parts[2];
            peers.put(id, new PeerInfo(grpcAddress, httpAddress));
        }
        return peers;
    }
}
