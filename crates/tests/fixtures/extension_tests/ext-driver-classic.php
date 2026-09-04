<?php
// This script is the front controller for the classic mode extension tests. Each `exec` call starts a new script.
echo 'ok:' . ($_GET['from'] ?? '?');
