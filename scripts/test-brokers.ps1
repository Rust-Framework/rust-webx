#requires -Version 5.1
[CmdletBinding()]
param(
    [switch]$KeepRunning,
    [switch]$ValidateOnly,
    [int]$TimeoutSeconds = 240
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$compose = Join-Path $root 'docker-compose.brokers.yml'

if ($ValidateOnly) {
    docker compose -f $compose config > $null
    if ($LASTEXITCODE -ne 0) {
        throw "docker-compose.brokers.yml is invalid (exit $LASTEXITCODE)"
    }
    Write-Output 'docker-compose.brokers.yml: OK'
    return
}

function Test-Port {
    param([string]$HostName, [int]$Port)
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $task = $client.ConnectAsync($HostName, $Port)
        return ($task.Wait(1000) -and $client.Connected)
    }
    catch {
        return $false
    }
    finally {
        $client.Dispose()
    }
}

function Wait-BrokerPort {
    param([string]$HostName, [int]$Port, [int]$Seconds)
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        if (Test-Port -HostName $HostName -Port $Port) {
            return
        }
        Start-Sleep -Milliseconds 500
    }
    throw "broker on ${HostName}:$Port did not open within $Seconds seconds"
}

$started = $false
try {
    docker compose -f $compose up -d --wait
    if ($LASTEXITCODE -ne 0) {
        throw "docker compose up failed (exit $LASTEXITCODE); is the Docker engine running?"
    }
    $started = $true

    # Redis/RabbitMQ/Kafka have healthchecks; NATS intentionally does not, so
    # this TCP gate is the authoritative readiness check for it (and a final
    # check for the others).
    foreach ($port in 6379, 4222, 5672, 9092) {
        Wait-BrokerPort -HostName '127.0.0.1' -Port $port -Seconds $TimeoutSeconds
    }

    $env:WEBX_REDIS_URL = 'redis://127.0.0.1:6379'
    $env:WEBX_NATS_URL = 'nats://127.0.0.1:4222'
    $env:WEBX_RABBITMQ_URL = 'amqp://guest:guest@127.0.0.1:5672/%2f'
    $env:WEBX_KAFKA_BROKERS = '127.0.0.1:9092'

    cargo test -p rust-webx-redis -p rust-webx-nats `
        -p rust-webx-rabbitmq -p rust-webx-kafka --tests -- --nocapture
    if ($LASTEXITCODE -ne 0) {
        throw "broker integration tests failed (exit $LASTEXITCODE)"
    }
}
finally {
    if ($started -and -not $KeepRunning) {
        docker compose -f $compose down -v
    }
}
