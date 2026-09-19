<?php
/**
 * The duck test, through a PHP Vault client.
 *
 * PHP is in this suite for a specific reason: ADR-0015 D1's operational rule
 * about applications reading secrets at runtime exists because of Laravel's
 * `config:cache`, which writes resolved configuration — secrets included — into
 * `bootstrap/cache` as plain PHP. A resolver on localhost does nothing for an
 * application that baked the secret into a file at deploy time. So the language
 * that motivated the rule gets a seat at the table.
 *
 * `csharpru/vault-php` is a third-party client rather than an official one,
 * which is also the point: it was written by reading Vault's HTTP API, so it
 * makes assumptions the official SDKs do not, and those assumptions are exactly
 * the sort of thing a compatibility layer trips over.
 */

require __DIR__ . '/vendor/autoload.php';

use Vault\Client;
use Vault\AuthenticationStrategies\TokenAuthenticationStrategy;
use GuzzleHttp\Client as GuzzleClient;
use Laminas\Diactoros\Uri;
use Laminas\Diactoros\RequestFactory;
use Laminas\Diactoros\StreamFactory;

$addr = getenv('VAULT_ADDR');
$token = getenv('VAULT_TOKEN') ?: null;

$failures = [];

function check(string $name, callable $fn): void {
    global $failures;
    try {
        $fn();
        echo "  ok    $name\n";
    } catch (\Throwable $e) {
        echo "  FAIL  $name: " . get_class($e) . ': ' . $e->getMessage() . "\n";
        $failures[] = $name;
    }
}

echo "php / csharpru/vault-php\n";

// v4 of this client takes its HTTP stack by injection: a PSR-7 URI, a PSR-18
// client, and PSR-17 factories. Spelling it out is the point — an application
// wiring Kallisto in will pass exactly these, and if any of Kallisto's
// responses are the wrong shape the failure surfaces through this stack rather
// than through a convenience wrapper that papers over it.
$client = new Client(
    new Uri($addr),
    new GuzzleClient(['http_errors' => false, 'timeout' => 10]),
    new RequestFactory(),
    new StreamFactory()
);
if ($token !== null && $token !== '') {
    // The library's own path: the strategy builds the Auth model and
    // `authenticate()` stores it. Constructing the token model by hand — which
    // this file did briefly — produces the wrong type and fails on the first
    // read, which is a good illustration of why this suite uses real clients
    // idiomatically rather than poking their internals.
    $client->setAuthenticationStrategy(new TokenAuthenticationStrategy($token));
    $client->authenticate();
}

check('read a secret', function () use ($client) {
    $response = $client->read('/secret/data/app/db');
    $data = $response->getData();
    if (($data['data']['password'] ?? null) !== 'duck-fixture-not-a-credential') {
        throw new RuntimeException('unexpected value: ' . json_encode($data));
    }
    if (($data['metadata']['version'] ?? 0) < 1) {
        throw new RuntimeException('no usable version metadata');
    }
});

check('read a nested secret', function () use ($client) {
    $data = $client->read('/secret/data/app/sub/deep')->getData();
    if (($data['data']['k'] ?? null) !== 'v') {
        throw new RuntimeException('unexpected value: ' . json_encode($data));
    }
});

check('list', function () use ($client) {
    // This library is inconsistent with itself: `read()` runs the path through
    // `buildPath()`, which prepends `/v1`, while `list()` sits on the base
    // class and sends the path verbatim. So the caller has to prepend it.
    //
    // Worth leaving visible rather than smoothing over, because it is the
    // reason a third-party client is in this suite at all: it was written by
    // reading Vault's HTTP API rather than by the people who built the server,
    // and it makes assumptions the official SDKs do not.
    $data = $client->list($client->buildPath('/secret/metadata/app'))->getData();
    $keys = $data['keys'] ?? [];
    if (!in_array('db', $keys, true)) {
        throw new RuntimeException('db missing from the listing: ' . json_encode($keys));
    }
    if (!in_array('sub/', $keys, true)) {
        throw new RuntimeException('no directory entry: ' . json_encode($keys));
    }
});

check('missing secret raises', function () use ($client) {
    try {
        $client->read('/secret/data/nope/nothing');
    } catch (\Throwable $e) {
        return;
    }
    throw new RuntimeException('a missing secret was served');
});

// The raw HTTP assertions. The client library has no write methods worth
// calling here, and what matters is the status code and the body an
// application's own HTTP layer would see.
function raw(string $method, string $path, ?string $token, ?array $body = null): array {
    $addr = getenv('VAULT_ADDR');
    $headers = ["Content-Type: application/json"];
    if ($token) { $headers[] = "X-Vault-Token: $token"; }
    $context = stream_context_create(['http' => [
        'method' => $method,
        'header' => implode("\r\n", $headers),
        'content' => $body === null ? '' : json_encode($body),
        'ignore_errors' => true,
        'timeout' => 5,
    ]]);
    $text = file_get_contents($addr . $path, false, $context);
    $status = 0;
    foreach ($http_response_header as $line) {
        if (preg_match('#^HTTP/\S+ (\d+)#', $line, $m)) { $status = (int) $m[1]; }
    }
    return [$status, json_decode($text, true)];
}

check('every write is refused with 403 and Vault\'s wording', function () use ($token) {
    $attempts = [
        ['PUT', '/v1/secret/data/app/db', ['data' => ['x' => 'y']]],
        ['POST', '/v1/secret/data/app/db', ['data' => ['x' => 'y']]],
        ['PATCH', '/v1/secret/data/app/db', ['data' => ['x' => 'y']]],
        ['DELETE', '/v1/secret/data/app/db', null],
        ['POST', '/v1/secret/delete/app/db', ['versions' => [1]]],
        ['POST', '/v1/secret/undelete/app/db', ['versions' => [1]]],
        ['POST', '/v1/secret/destroy/app/db', ['versions' => [1]]],
    ];
    foreach ($attempts as [$method, $path, $body]) {
        [$status, $decoded] = raw($method, $path, $token, $body);
        if ($status !== 403) {
            throw new RuntimeException("$method $path answered $status, want 403");
        }
        if (($decoded['errors'][0] ?? null) !== 'permission denied') {
            throw new RuntimeException("$method $path: wrong body " . json_encode($decoded));
        }
    }
});

check('sys/health carries the resolver fields', function () use ($token) {
    [$status, $body] = raw('GET', '/v1/sys/health', $token);
    if ($status !== 200) { throw new RuntimeException("health answered $status"); }
    if (($body['sealed'] ?? true) !== false) { throw new RuntimeException('reported sealed'); }
    if (($body['kallisto_file_version'] ?? 0) < 1) {
        throw new RuntimeException('no file version: ' . json_encode($body));
    }
});

if ($failures) {
    echo "\n" . count($failures) . " failure(s): " . implode(', ', $failures) . "\n";
    exit(1);
}
echo "\nall php checks passed\n";
