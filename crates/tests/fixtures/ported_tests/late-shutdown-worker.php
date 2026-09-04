<?php
// Bootstrap registers two shutdown functions, and code after the loop registers one. PHP runs the bootstrap functions first at the end of the cycle.
register_shutdown_function(static function (): void {
    \Rapira\log('sd boot-a');
});
register_shutdown_function(static function (): void {
    \Rapira\log('sd boot-b');
});
$handler = static function (): void {
    echo 'ok';
};
while (\Rapira\handle_request($handler)) {
}
register_shutdown_function(static function (): void {
    \Rapira\log('sd late');
});
