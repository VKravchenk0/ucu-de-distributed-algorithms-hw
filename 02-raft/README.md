# HW2: Raft protocol

Часткова реалізація алгоритму консенсусу Raft (https://raft.github.io/).

Покриті частини 
- Leader election
- Log replication

Для цілей цього проєкту, згідно з завданням, були зроблені спрощення:
- Стан нод не персистентний
- Відсутнє додавання нових нод до кластеру
- Відсутній log compaction/snapshotting

## 1. Quickstart
Проєкт доступний до запуску в [dev-контейнері](https://containers.dev/). 

Acceptance test, описаний в завданні, як і інші тести, імплементовані у вигляді інтеграційних тестів, в яких ноди підіймаються за допомогою [Testcontainers](https://testcontainers.com/)

Запуск тестів:
```bash
mvn test      # unit-тести
mvn verify    # інтеграційні тести (потрібен docker daemon для роботи testcontainers)
```

Інтеграційні тести:
| Назва | Роль |
| --- | --- |
| [AcceptanceTestIT](src/test/java/ua/vk/ucu/dads/it/AcceptanceTestIT.java)  | Тест, описаний в завданні |
| [LeaderElectionIT](src/test/java/ua/vk/ucu/dads/it/LeaderElectionIT.java)  | Окремий тест на leader election |
| [ReplicationIT](src/test/java/ua/vk/ucu/dads/it/ReplicationIT.java)  | Окремий тест на log replication |

## 2. Опис рішення
### 2.1 Загальна архітектура

Кожна нода - це один JVM-процес, що виставляє два інтерфейси:

| Інтерфейс | Порт | Протокол | Методи |
| --- | --- | --- | --- |
| Клієнтський API | `7000` | HTTP/JSON (Javalin) | `POST /command`, `GET /log`, `GET /state`, `GET /state-machine`, `GET /health` |
| API для Raft-нод | `6001` | gRPC/protobuf | `RequestVote`, `AppendEntries` |

### 2.2 Модель concurrency - event loop
`RaftNode` володіє всім станом Raft і змінює його виключно в однопотоковому `ScheduledExecutorService` -
[RaftNode.java:54](src/main/java/ua/vk/ucu/dads/raft/RaftNode.java#L54).

* **Вхідні RPC** (`handleRequestVote` / `handleAppendEntries`) передаються в event loop потоком обробника gRPC, який блокуюче обробляться з таймаутом в 500мс
  ([RaftNode.java:129-138](src/main/java/ua/vk/ucu/dads/raft/RaftNode.java#L129-L138)).
* **Вихідні RPC** виконуються в окремому пулі віртуальних потоків, тому повільна нода
 ніколи не блокує event loop.
* **Таймери** (таймаут виборів, heartbeat) - це заплановані задачі в тому самому event loop.
* **Читання з інших потоків** (HTTP `GET`-ендпоінти для запитів від клієнта) відбувається через `volatile`-снепшот `ServerState`, який перепубліковується після кожної зміни стану (`publishSnapshot()`), тож жоден читач ніколи не конкурує з циклом.

### 2.3 Конфігурація

Конфігурація кластеру статична, вказується змінними середовища ([NodeConfig.java](src/main/java/ua/vk/ucu/dads/config/NodeConfig.java)):

```
NODE_ID=1
PEERS=2=host2:6001:7000,3=host3:6001:7000      # id=хост:grpcПорт:httpПорт
```

### 2.4 Структура коду

| Пакет | Вміст |
| --- | --- |
| `raft` | `RaftNode` (сам алгоритм), `ServerState`, `ServerStatus`, `CommandResult` |
| `raft.log` | `LogStore`, `LogEntry` |
| `replication` | gRPC сервер/клієнт, `PeerRpcClient`, `ProtoMapper` |
| `statemachine` | `StateMachine`, `Command`, `UnknownKeyException` |
| `clientapi` | `HttpApi` — HTTP-роути для комунікації з клієнтом |
| `config` | `NodeConfig` — конфігурація `NODE_ID` / `PEERS` |
| `src/main/proto` | `replication.proto` — два RPC та їхні повідомлення |

### 2.5 Точки входу

#### 2.5.1 Leader election
| Метод | Роль |
| --- | --- |
| [RaftNode.resetElectionTimer()](src/main/java/ua/vk/ucu/dads/raft/RaftNode.java#L142) | планує таймаут виборів, скасовуючи попередній. Викликається при старті ноди, при наданні голосу та при кожному коректному `AppendEntries`. |
| [RaftNode.startElectionRound()](src/main/java/ua/vk/ucu/dads/raft/RaftNode.java#L154) | Безпосередньо запускає процес голосування |
| [RaftNode.handleRequestVoteInternal()](src/main/java/ua/vk/ucu/dads/raft/RaftNode.java#L364) | Точка входу на стороні фоловера - обробляє запити від інших кандидатів |


#### 2.5.2 Log replication
| Метод | Роль |
| --- | --- |
| [RaftNode.submitCommand()](src/main/java/ua/vk/ucu/dads/raft/RaftNode.java#L113) | Точка входу на боці лідера: додає запис в лог, ставить клієнтський future на цей запис, реплікує (`replicateToAllPeers()`), потім намагається зробити commit (`advanceCommitIndex()`) та apply (`applyCommitted()`) |
| [RaftNode.handleAppendEntriesInternal()](src/main/java/ua/vk/ucu/dads/raft/RaftNode.java#L391) | Точка входу на боці фоловера: перевірка консистентності за `prevLogIndex`/`prevLogTerm`; при конфлікті термінів - підчистка логу через `truncateFrom(idx)`. Далі append нового запису, commit і apply. |