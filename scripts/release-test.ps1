[CmdletBinding()]
param(
    [switch]$LiveTelegram,
    [switch]$SkipBrowser,
    [switch]$SkipDocker
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Push-Location $repo
try {
    function Invoke-Gate([string]$Label, [scriptblock]$Command) {
        Write-Host "== $Label ==" -ForegroundColor Cyan
        & $Command
        if ($LASTEXITCODE -ne 0) {
            throw "$Label failed with exit code $LASTEXITCODE"
        }
    }

    Invoke-Gate 'Rust format' { cargo fmt --all -- --check }
    Invoke-Gate 'Rust clippy' { cargo clippy --workspace --all-targets --all-features -- -D warnings }
    Invoke-Gate 'Rust tests and deterministic recovery matrix' { cargo test --workspace --all-features }

    if (-not $SkipDocker) {
        Invoke-Gate 'Docker and Compose smoke tests' { cargo test --test docker_packaging_smoke --all-features }
    }

    Push-Location (Join-Path $repo 'frontend')
    try {
        Invoke-Gate 'Frontend type check' { npm run check }
        Invoke-Gate 'Frontend production build' { npm run build }
        if (-not $SkipBrowser) {
            if (-not (Get-Command npx -ErrorAction SilentlyContinue)) {
                throw 'npx is required for the Playwright browser suite'
            }
            Invoke-Gate 'Playwright browser suite' { npm run test:e2e }
        }
    }
    finally {
        Pop-Location
    }

    if (Get-Command cargo-audit -ErrorAction SilentlyContinue) {
        Invoke-Gate 'Cargo audit' { cargo audit }
    }
    else {
        Write-Warning 'cargo-audit is not installed; dependency audit was skipped.'
    }
    if (Get-Command cargo-deny -ErrorAction SilentlyContinue) {
        Invoke-Gate 'Cargo deny' { cargo deny check }
    }
    else {
        Write-Warning 'cargo-deny is not installed; dependency policy check was skipped.'
    }

    if ($LiveTelegram) {
        if (-not $env:TELEGRAM_LIVE_TESTS) { $env:TELEGRAM_LIVE_TESTS = '1' }
        Invoke-Gate 'Isolated live Telegram drill' {
            cargo test --test telegram_live --all-features -- --ignored --nocapture
        }
    }
    else {
        Write-Host 'Live Telegram drill skipped. Use -LiveTelegram with isolated live-test paths.' -ForegroundColor Yellow
    }

    Write-Host 'Release test suite passed.' -ForegroundColor Green
}
finally {
    Pop-Location
}
