<?php
// An uncaught exception occurs before the request loop. Each worker fails to start, and the generation 0 pool becomes unhealthy.
throw new RuntimeException('fatal-worker: intentional bootstrap failure');
