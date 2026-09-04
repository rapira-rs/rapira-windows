<?php
class State
{
    public static int $n = 0;
}
$handler = static function (): void {
    header('Content-Type: text/plain');
    echo "count=" . State::$n . " BEFORE";
    \rapira_finish_request();   // php_output_end_all() and Context::finish() flush and close the stream.
    echo " AFTER";             // The closed stream discards this output.
    State::$n++;               // Work continues after the response. The next request reads this value.
};
while (\Rapira\handle_request($handler)) {
    gc_collect_cycles();
}
