package ua.vk.ucu.dads.config;

import java.util.Arrays;
import java.util.List;
import java.util.Map;

public record NodeConfig(boolean isMaster, List<String> secondaryAddresses, String masterUrl) {

    public static NodeConfig fromEnv() {
        return from(System.getenv());
    }

    public static NodeConfig from(Map<String, String> env) {
        boolean isMaster = Boolean.parseBoolean(env.getOrDefault("IS_MASTER", "false"));
        List<String> secondaries = parseAddresses(env.get("SECONDARY_ADDRESSES"));
        String masterUrl = env.getOrDefault("MASTER_URL", "");
        return new NodeConfig(isMaster, secondaries, masterUrl);
    }

    private static List<String> parseAddresses(String raw) {
        if (raw == null || raw.isBlank()) {
            return List.of();
        }
        return Arrays.stream(raw.split(","))
                .map(String::trim)
                .filter(s -> !s.isEmpty())
                .toList();
    }
}
