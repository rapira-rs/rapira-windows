<?php

// exit() in a serializer starts PHP exit processing. log() must let the exit continue.
final class Quitter implements \JsonSerializable
{
	public function jsonSerialize(): mixed
	{
		echo 'quitting';
		exit;
	}
}

\Rapira\log('quit', \Rapira\LogLevel::Info, ['q' => new Quitter()]);
echo ' after-log';
