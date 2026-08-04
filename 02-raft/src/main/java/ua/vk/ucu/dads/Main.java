package ua.vk.ucu.dads;

import io.grpc.Server;
import io.grpc.ServerBuilder;
import io.javalin.Javalin;
import ua.vk.ucu.dads.replication.ReplicationClient;
import ua.vk.ucu.dads.replication.ReplicationServer;

public class Main {
    private static final int HTTP_PORT = 7000;
    private static final int GRPC_PORT = 6001;

    public static void main(String[] args) throws Exception {
        NodeConfig config = NodeConfig.fromEnv();
        LogStore logStore = new LogStore();

        Server grpcServer = ServerBuilder.forPort(GRPC_PORT)
                .addService(new ReplicationServer(logStore))
                .build()
                .start();

        ReplicationClient replicationClient = new ReplicationClient(config.secondaryAddresses());

        Javalin app = Javalin.create();
        new HttpApi(config, logStore, replicationClient).register(app);
        app.start(HTTP_PORT);

        grpcServer.awaitTermination();
    }
}
