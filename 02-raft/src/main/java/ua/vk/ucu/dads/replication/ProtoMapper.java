package ua.vk.ucu.dads.replication;

import ua.vk.ucu.dads.grpc.Command;
import ua.vk.ucu.dads.raft.log.LogEntry;
import ua.vk.ucu.dads.statemachine.UnknownKeyException;

import java.util.List;
import java.util.stream.Collectors;

/** Converts between the domain {@link LogEntry}/{@link ua.vk.ucu.dads.statemachine.Command} types and their protobuf wire counterparts. */
public final class ProtoMapper {
    private ProtoMapper() {
    }

    public static ua.vk.ucu.dads.grpc.LogEntry toProto(LogEntry entry) {
        return ua.vk.ucu.dads.grpc.LogEntry.newBuilder()
                .setTerm(entry.term())
                .setCommand(toProto(entry.command()))
                .build();
    }

    public static LogEntry fromProto(ua.vk.ucu.dads.grpc.LogEntry proto) {
        return new LogEntry(proto.getTerm(), fromProto(proto.getCommand()));
    }

    public static Command toProto(ua.vk.ucu.dads.statemachine.Command command) {
        return Command.newBuilder()
                .setKey(command.key())
                .setAction(toProto(command.action()))
                .setValue(command.value())
                .build();
    }

    public static ua.vk.ucu.dads.statemachine.Command fromProto(Command proto) {
        return new ua.vk.ucu.dads.statemachine.Command(proto.getKey(), fromProto(proto.getAction()), proto.getValue());
    }

    public static Command.Action toProto(ua.vk.ucu.dads.statemachine.Command.Action action) {
        return switch (action) {
            case SET -> Command.Action.SET;
            case ADD -> Command.Action.ADD;
            case SUBTRACT -> Command.Action.SUBTRACT;
        };
    }

    public static ua.vk.ucu.dads.statemachine.Command.Action fromProto(Command.Action action) {
        return switch (action) {
            case SET -> ua.vk.ucu.dads.statemachine.Command.Action.SET;
            case ADD -> ua.vk.ucu.dads.statemachine.Command.Action.ADD;
            case SUBTRACT -> ua.vk.ucu.dads.statemachine.Command.Action.SUBTRACT;
            case UNRECOGNIZED -> throw new UnknownKeyException("unrecognized action");
        };
    }

    public static List<ua.vk.ucu.dads.grpc.LogEntry> toProto(List<LogEntry> entries) {
        return entries.stream().map(ProtoMapper::toProto).collect(Collectors.toList());
    }
}
