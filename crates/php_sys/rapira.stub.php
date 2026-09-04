<?php

/** @generate-class-entries */

namespace {
    /** Flushes the response early in classic and worker modes. The script can continue after the call. This function throws in dispatcher mode because the Exchange methods finalize the response. */
    function rapira_finish_request(): bool {}
}

namespace Rapira {
    enum LogLevel
    {
        case Error;
        case Warning;
        case Info;
        case Debug;
        case Trace;
    }

    /** The process mode from `[pool].mode` in rapira.toml. */
    enum Mode
    {
        case Classic;
        case Worker;
        case Dispatcher;
    }

    /** Represents one unit of work from a dispatcher. The host creates it. The concrete type provides the finalization methods. */
    interface Work
    {
        public function isFinalized(): bool;

        public function isCancelled(): bool;

        /** Reports an unfinalized unit to the host when code drops the last reference. The host then fails the unit. A finalized, discarded, or referenced unit is not changed. */
        public function __destruct();
    }

    /** Provides an immutable counter snapshot for monitoring. */
    interface DispatcherInfo
    {
        public function pendingCount(): int;

        public function activeCount(): int;
    }

    /** Defines the plugin interface that this worker pool serves. Plugins return their own types from receive(), tryReceive(), and getInfo(). */
    interface Dispatcher
    {
        public function name(): string;

        /**
         * Returns immediately. A null value means that no work is available.
         *
         * @throws Exception\ClosedException
         */
        public function tryReceive(): ?Work;

        /**
         * @param int $timeout Timeout in microseconds. A value of -1 waits without a limit. A value of 0 returns immediately.
         * @throws Exception\TimeoutException
         * @throws Exception\ClosedException
         */
        public function receive(int $timeout = -1): Work;

        public function getInfo(): DispatcherInfo;
    }

    /**
     * Represents an IP endpoint. UnixAddress is the other type in the address union.
     *
     * @strict-properties
     * @not-serializable
     */
    final readonly class InetAddress
    {
        public string $ip;
        public int $port;

        public function __construct(string $ip, int $port) {}
    }

    /**
     * Represents a Unix domain socket endpoint. $path is null for an unnamed peer.
     *
     * @strict-properties
     * @not-serializable
     */
    final readonly class UnixAddress
    {
        public ?string $path;

        public function __construct(?string $path) {}
    }

    /** Returns the process mode. The value does not change during the process. */
    function get_mode(): Mode {}

    /**
     * Returns the same instance during the process.
     *
     * @throws Exception\NoDispatcherError Called outside dispatcher mode.
     */
    function get_dispatcher(): Dispatcher {}

    /**
     * Passes one job to $handler. The handler reads the superglobals and responds through echo or header(). A false value means that the worker is draining. Exit the loop after a false value. Call this function only from the top level of the boot script. Calls from a shutdown function or destructor have undefined behavior.
     *
     * @throws Exception\NotInWorkerModeError Called outside worker mode.
     */
    function handle_request(callable $handler): bool {}

    function get_version(): string {}

    /**
     * Queues a record to the host under the `app` target. The function returns immediately and does not throw. The serializer preserves the structure of a \Throwable in $context. json_encode() reads only public state, but an exception stores its state in private properties.
     */
    function log(string $message, LogLevel $level = LogLevel::Info, array $context = []): void {}
}
