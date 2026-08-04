package ua.vk.ucu.dads;

import io.grpc.Server;
import io.grpc.ServerBuilder;
import io.javalin.Javalin;
import ua.vk.ucu.dads.clientapi.HttpApi;
import ua.vk.ucu.dads.config.NodeConfig;
import ua.vk.ucu.dads.log.LogStore;
import ua.vk.ucu.dads.raft.RaftNode;
import ua.vk.ucu.dads.replication.ReplicationClient;
import ua.vk.ucu.dads.replication.ReplicationServer;

public class Main {
    private static final int HTTP_PORT = 7000;
    private static final int GRPC_PORT = 6001;

    public static void main(String[] args) throws Exception {
        NodeConfig config = NodeConfig.fromEnv();
        LogStore logStore = new LogStore();

        ReplicationClient replicationClient = new ReplicationClient(config.peers());
        RaftNode raftNode = new RaftNode(config, logStore, replicationClient);

        Server grpcServer = ServerBuilder.forPort(GRPC_PORT)
                .addService(new ReplicationServer(raftNode))
                .build()
                .start();

        createClientApi(raftNode, config, logStore, replicationClient);

        raftNode.start();

        grpcServer.awaitTermination();
    }

    private static void createClientApi(RaftNode raftNode, NodeConfig config, LogStore logStore, ReplicationClient replicationClient) {
        Javalin app = Javalin.create();
        new HttpApi(raftNode, config, logStore, replicationClient).register(app);
        app.start(HTTP_PORT);
    }
}
