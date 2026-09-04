<?php
// RFC 3875 defines `Status: NNN` as a CGI response field. PHP keeps it as a regular field. The HTTP server consumes the field and sets the response status. https://www.rfc-editor.org/rfc/rfc3875#section-6.3.3 https://github.com/php/php-src/blob/dab13a022a54f8bc03302f93ccb6484907ec1245/main/SAPI.c#L677-L777
$handler = static function (): void {
    header('Status: 404 Not Found');
    header('X-Keep: kept');
    echo 'body';
};
while (\Rapira\handle_request($handler)) {
}
