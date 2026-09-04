<?php

/** @generate-class-entries */

namespace Rapira\Exception {
    /** Identifies all exceptions and errors that Rapira throws. */
    interface RapiraThrowable extends \Throwable
    {
    }

    /** Indicates that no more work will arrive. Each subsequent receive call throws this exception. */
    class ClosedException extends \RuntimeException implements RapiraThrowable
    {
    }

    /** Indicates that the wait duration expired. This exception does not indicate a closed dispatcher. */
    class TimeoutException extends \RuntimeException implements RapiraThrowable
    {
    }

    /** Indicates that the host closed the unit after a deadline, server drain, or client disconnect. */
    class WorkDiscardedException extends \RuntimeException implements RapiraThrowable
    {
    }

    /** Indicates a call to get_dispatcher() outside dispatcher mode. */
    class NoDispatcherError extends \Error implements RapiraThrowable
    {
    }

    /** Indicates a call to handle_request() outside worker mode. */
    class NotInWorkerModeError extends \Error implements RapiraThrowable
    {
    }

    /** Indicates that this worker already finalized the unit. */
    class AlreadyFinalizedError extends \Error implements RapiraThrowable
    {
    }
}
