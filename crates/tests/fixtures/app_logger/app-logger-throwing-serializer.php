<?php

// log() must contain an exception from jsonSerialize() in a context value and continue the script.
final class Bomb implements \JsonSerializable
{
	public function jsonSerialize(): mixed
	{
		throw new \RuntimeException('serializer bomb');
	}
}

try {
	\Rapira\log('bombed', \Rapira\LogLevel::Error, ['keep' => 'visible', 'bomb' => new Bomb()]);
	echo 'logged';
} catch (\Throwable $e) {
	echo 'escaped:', $e->getMessage();
}
