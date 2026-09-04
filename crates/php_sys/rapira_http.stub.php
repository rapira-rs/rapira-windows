<?php

/** @generate-class-entries */

namespace Rapira\Http {
    /**
     * Contains the result of the TLS handshake. The certificate fields describe the client certificate. They are null when the client does not provide a certificate.
     *
     * @strict-properties
     * @not-serializable
     */
    final readonly class Tls
    {
        public string $version;
        public string $cipher;
        public ?string $negotiatedProtocol;
        public ?string $requestedServerName;
        public ?string $certSerial;
        public ?string $certOrganization;
        public ?string $certFingerprint;

        public function __construct(
            string $version,
            string $cipher,
            ?string $negotiatedProtocol,
            ?string $requestedServerName,
            ?string $certSerial,
            ?string $certOrganization,
            ?string $certFingerprint,
        ) {}
    }

    /**
     * Represents a field part of a multipart/form-data body. Its Content-Disposition field has no `filename` parameter. The host stores the part in memory.
     *
     * @strict-properties
     * @not-serializable
     */
    final readonly class FormField
    {
        public string $name;
        public string $value;
        /** @var array<string, list<string>> */
        public array $headers;

        public function __construct(string $name, string $value, array $headers) {}
    }

    /**
     * Represents a file part of a multipart/form-data body. The host writes the bytes to $tmpPath. Rename the file to retain it after the exchange finalizes.
     *
     * @strict-properties
     * @not-serializable
     */
    final readonly class UploadedFile
    {
        public string $name;
        public string $clientFilename;
        public ?string $clientMediaType;
        /** @var array<string, list<string>> */
        public array $headers;
        public string $tmpPath;
        public int $size;

        public function __construct(
            string $name,
            string $clientFilename,
            ?string $clientMediaType,
            array $headers,
            string $tmpPath,
            int $size,
        ) {}
    }

    /**
     * Represents a multipart/form-data body that the host parsed during the upload.
     *
     * @strict-properties
     * @not-serializable
     */
    final readonly class Multipart
    {
        /** @var list<FormField> */
        public array $fields;
        /** @var list<UploadedFile> */
        public array $files;

        public function __construct(array $fields, array $files) {}
    }

    /**
     * Contains request data from the host. This object does not implement the PSR-7 HTTP message interfaces: https://www.php-fig.org/psr/psr-7/
     *
     * @strict-properties
     * @not-serializable
     */
    final readonly class Request
    {
        public string $method;
        public string $uri;
        public string $target;
        public ?string $authority;
        public string $protocol;
        /** @var array<string, list<string>> */
        public array $headers;
        public string|Multipart $body;
        public \Rapira\InetAddress|\Rapira\UnixAddress $remote;
        public \Rapira\InetAddress|\Rapira\UnixAddress $server;
        public ?Tls $tls;
        public float $receivedAt;

        public function __construct(
            string $method,
            string $uri,
            string $target,
            ?string $authority,
            string $protocol,
            array $headers,
            string|Multipart $body,
            \Rapira\InetAddress|\Rapira\UnixAddress $remote,
            \Rapira\InetAddress|\Rapira\UnixAddress $server,
            ?Tls $tls,
            float $receivedAt,
        ) {}
    }

    /** Provides HTTP-specific dispatcher counters. */
    interface HttpDispatcherInfo extends \Rapira\DispatcherInfo
    {
    }

    /** Represents one request and response exchange. It contains request data and response methods. */
    interface Exchange extends \Rapira\Work
    {
        public function getRequest(): Request;

        /**
         * $headers has the type array<string, list<string>>. Each value has a separate list entry.
         *
         * @throws Exception\HeadAlreadyWrittenError
         * @throws \Rapira\Exception\WorkDiscardedException
         * @throws \ValueError
         */
        public function writeHead(int $status, array $headers = []): void;

        /**
         * @throws Exception\ContentLengthExceededError
         * @throws \Rapira\Exception\AlreadyFinalizedError
         * @throws \Rapira\Exception\WorkDiscardedException
         */
        public function writeBody(string $content, bool $eos = true): void;

        /**
         * @throws Exception\FileNotSendableException
         * @throws Exception\ContentLengthExceededError
         * @throws \Rapira\Exception\AlreadyFinalizedError
         * @throws \Rapira\Exception\WorkDiscardedException
         * @throws \ValueError
         */
        public function sendFile(string $path, int $offset = 0, ?int $length = null, bool $eos = true): void;

        /**
         * $trailers has the same type as $headers in writeHead().
         *
         * @throws Exception\HeadNotWrittenError
         * @throws \Rapira\Exception\AlreadyFinalizedError
         * @throws \Rapira\Exception\WorkDiscardedException
         * @throws \ValueError
         */
        public function writeTrailers(array $trailers): void;

        /**
         * @throws \Rapira\Exception\AlreadyFinalizedError
         * @throws \Rapira\Exception\WorkDiscardedException
         */
        public function flush(): void;
    }

    /** Defines the HTTP plugin dispatcher that \Rapira\get_dispatcher() returns. */
    interface HttpDispatcher extends \Rapira\Dispatcher
    {
        public function tryReceive(): ?Exchange;

        public function receive(int $timeout = -1): Exchange;

        public function getInfo(): HttpDispatcherInfo;
    }
}

namespace Rapira\Http\Exception {
    /** Indicates that a body write exceeded the declared Content-Length value. */
    class ContentLengthExceededError extends \Error implements \Rapira\Exception\RapiraThrowable
    {
    }

    /** Indicates that the final response headers were already written. */
    class HeadAlreadyWrittenError extends \Error implements \Rapira\Exception\RapiraThrowable
    {
    }

    /** Indicates a trailer section without final response headers. */
    class HeadNotWrittenError extends \Error implements \Rapira\Exception\RapiraThrowable
    {
    }

    /** Indicates that the host cannot send the file. sendFile() throws this exception before it writes data. */
    class FileNotSendableException extends \RuntimeException implements \Rapira\Exception\RapiraThrowable
    {
    }
}

namespace Rapira\Internal\Http {
    /**
     * Implements \Rapira\Http\HttpDispatcher. The host creates this object.
     *
     * @strict-properties
     * @not-serializable
     */
    final class Dispatcher implements \Rapira\Http\HttpDispatcher
    {
        /** The host creates this object. Obtain it from \Rapira\get_dispatcher(). */
        private function __construct() {}

        public function name(): string {}

        public function tryReceive(): ?\Rapira\Http\Exchange {}

        public function receive(int $timeout = -1): \Rapira\Http\Exchange {}

        public function getInfo(): \Rapira\Http\HttpDispatcherInfo {}
    }

    /**
     * @strict-properties
     * @not-serializable
     */
    final class DispatcherInfo implements \Rapira\Http\HttpDispatcherInfo
    {
        /** The host creates this object. */
        private function __construct() {}

        public function pendingCount(): int {}

        public function activeCount(): int {}
    }

    /**
     * @strict-properties
     * @not-serializable
     */
    final class Exchange implements \Rapira\Http\Exchange
    {
        /** The host creates this object. */
        private function __construct() {}

        public function isFinalized(): bool {}

        public function isCancelled(): bool {}

        public function getRequest(): \Rapira\Http\Request {}

        public function writeHead(int $status, array $headers = []): void {}

        public function writeBody(string $content, bool $eos = true): void {}

        public function sendFile(string $path, int $offset = 0, ?int $length = null, bool $eos = true): void {}

        public function writeTrailers(array $trailers): void {}

        public function flush(): void {}

        public function __destruct() {}
    }
}
