<?php

// Reports memory growth after 1000 logs with computed Throwable properties.

class GetMessage extends \Exception
{
    public function __construct()
    {
        parent::__construct();
        unset($this->message);
    }

    public function __get($name)
    {
        // Create a new string for each property read.
        return "computed $name";
    }
}

class HookedMessage extends \Exception
{
    protected $message {
        get => '[app] ' . $this->message;
    }
}

$d = \Rapira\get_dispatcher();
try {
    while (true) {
        $ex = $d->receive();
        parse_str(parse_url($ex->getRequest()->target, PHP_URL_QUERY) ?: '', $q);
        $e = match ($q['case'] ?? '') {
            'get' => new GetMessage(),
            'hook' => new HookedMessage('m'),
        };
        // The first call allocates state that later calls reuse.
        \Rapira\log('probe', \Rapira\LogLevel::Error, ['e' => $e]);
        $before = memory_get_usage();
        for ($i = 0; $i < 1000; $i++) {
            \Rapira\log('probe', \Rapira\LogLevel::Error, ['e' => $e]);
        }
        $ex->writeBody((string) (memory_get_usage() - $before));
    }
} catch (\Rapira\Exception\ClosedException) {
}
