param(
    [Parameter(Mandatory = $true)][string]$ArchivePath,
    [Parameter(Mandatory = $true)][string]$ChecksumPath
)

$ErrorActionPreference = 'Stop'
$testArchivePath = (Resolve-Path -LiteralPath $ArchivePath).Path
$testChecksumPath = (Resolve-Path -LiteralPath $ChecksumPath).Path
$version = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value
if (-not $version) { throw 'Could not read the package version from Cargo.toml.' }
$checksumLine = (Get-Content -LiteralPath $testChecksumPath -Raw).Trim()
$expectedHash = ($checksumLine -split '\s+')[0].ToLowerInvariant()
$actualHash = (Get-FileHash -LiteralPath $testArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualHash -ne $expectedHash) { throw "Package checksum mismatch: expected $expectedHash, got $actualHash." }
Write-Host "Verified package target=x86_64-pc-windows-msvc platform=Windows/$env:PROCESSOR_ARCHITECTURE archive=$testArchivePath sha256=$actualHash"

$testRoot = Join-Path $env:TEMP "GitScry package test with spaces $([guid]::NewGuid().ToString('N'))"
$archiveRoot = Join-Path $testRoot 'archive'
$installDir = Join-Path $testRoot 'installed package'
$rollbackDir = Join-Path $testRoot 'rollback package'
$smokeRepo = Join-Path $testRoot 'smoke repository'
New-Item -ItemType Directory -Path $archiveRoot, $smokeRepo | Out-Null

try {
    Expand-Archive -LiteralPath $testArchivePath -DestinationPath $archiveRoot
    $manifestPath = Join-Path $archiveRoot "runtime/manifests/$version.json"
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    $runtimeLibrary = Join-Path (Join-Path $archiveRoot 'runtime' $manifest.runtime_id) $manifest.library
    $executable = Join-Path $archiveRoot 'gitscry.exe'
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf) -or
        -not (Test-Path -LiteralPath $runtimeLibrary -PathType Leaf)) {
        throw 'The cargo-dist archive is missing its executable or pinned ONNX Runtime library.'
    }
    foreach ($file in $manifest.files) {
        $path = Join-Path (Join-Path $archiveRoot 'runtime' $manifest.runtime_id) $file.name
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "The cargo-dist archive is missing runtime file $($file.name)."
        }
        if ((Get-Item -LiteralPath $path).Length -ne $file.bytes -or
            (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $file.sha256) {
            throw "The cargo-dist archive contains an invalid runtime file $($file.name)."
        }
    }

    function Invoke-WebRequest {
        param([string]$Uri, [string]$OutFile)
        $source = if ($Uri.EndsWith('.sha256')) { $testChecksumPath } else { $testArchivePath }
        Copy-Item -LiteralPath $source -Destination $OutFile
    }

    $installer = Join-Path $PSScriptRoot 'gitscry-installer.ps1'
    $env:GITSCRY_INSTALL_DIR = $installDir
    & $installer
    $installedExecutable = Join-Path $installDir 'gitscry.exe'
    $installedManifest = Join-Path $installDir "runtime/manifests/$version.json"
    if (-not (Test-Path -LiteralPath $installedExecutable -PathType Leaf) -or
        -not (Test-Path -LiteralPath $installedManifest -PathType Leaf) -or
        -not (Test-Path -LiteralPath (Join-Path $installDir "runtime/$($manifest.runtime_id)/$($manifest.library)") -PathType Leaf)) {
        throw 'The Windows installer did not install the executable and runtime package.'
    }

    $rollbackExecutable = Join-Path $rollbackDir 'gitscry.exe'
    $oldRuntime = Join-Path $rollbackDir 'runtime/ort-previous'
    $oldManifests = Join-Path $rollbackDir 'runtime/manifests'
    New-Item -ItemType Directory -Path $oldRuntime, $oldManifests | Out-Null
    Set-Content -LiteralPath $rollbackExecutable -Value 'previous executable' -NoNewline
    Set-Content -LiteralPath (Join-Path $oldRuntime 'onnxruntime.dll') -Value 'previous runtime' -NoNewline
    Set-Content -LiteralPath (Join-Path $oldManifests 'previous.json') -Value '{"runtime_id":"ort-previous"}' -NoNewline

    $script:moveItemCalls = 0
    function Move-Item {
        param([string]$LiteralPath, [string]$Destination)
        $script:moveItemCalls++
        if ($script:moveItemCalls -eq 2) { throw 'Injected executable publication failure.' }
        Microsoft.PowerShell.Management\Move-Item -LiteralPath $LiteralPath -Destination $Destination
    }
    $failure = $null
    try {
        $env:GITSCRY_INSTALL_DIR = $rollbackDir
        & $installer
    } catch {
        $failure = $_.Exception.Message
    } finally {
        Remove-Item Function:\Move-Item -ErrorAction SilentlyContinue
    }
    if ($failure -notlike '*Injected executable publication failure.*') {
        throw "The Windows installer did not surface the injected replacement failure: $failure"
    }
    if ((Get-Content -LiteralPath $rollbackExecutable -Raw) -ne 'previous executable' -or
        -not (Test-Path -LiteralPath (Join-Path $oldManifests 'previous.json') -PathType Leaf) -or
        -not (Test-Path -LiteralPath (Join-Path $oldRuntime 'onnxruntime.dll') -PathType Leaf)) {
        throw 'A failed update did not preserve the previous executable and runtime.'
    }

    function Invoke-Git {
        param([string]$Directory, [string[]]$Arguments)
        & git -C $Directory @Arguments
        if ($LASTEXITCODE -ne 0) { throw "git $Arguments failed with exit code $LASTEXITCODE." }
    }
    Invoke-Git $smokeRepo @('init', '-q', '-b', 'main')
    Set-Content -LiteralPath (Join-Path $smokeRepo 'fixture.txt') -Value 'first packaged inference'
    Invoke-Git $smokeRepo @('add', 'fixture.txt')
    Invoke-Git $smokeRepo @('-c', 'user.name=Smoke', '-c', 'user.email=smoke@example.invalid', 'commit', '-qm', 'initial')
    Push-Location $smokeRepo
    try {
        & $installedExecutable index --semantic
        if ($LASTEXITCODE -ne 0) { throw "Packaged semantic inference failed with exit code $LASTEXITCODE." }
        foreach ($name in @('HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy')) {
            [Environment]::SetEnvironmentVariable($name, 'http://127.0.0.1:9', 'Process')
        }
        $null = & $installedExecutable index --semantic
        if ($LASTEXITCODE -ne 0) { throw "Packaged no-op semantic indexing failed with exit code $LASTEXITCODE." }
        Add-Content -LiteralPath (Join-Path $smokeRepo 'fixture.txt') -Value 'second offline inference'
        Invoke-Git $smokeRepo @('add', 'fixture.txt')
        Invoke-Git $smokeRepo @('-c', 'user.name=Smoke', '-c', 'user.email=smoke@example.invalid', 'commit', '-qm', 'second')
        & $installedExecutable index --semantic
        if ($LASTEXITCODE -ne 0) { throw "Cached offline semantic inference failed with exit code $LASTEXITCODE." }
        $searchOutput = & $installedExecutable search 'second offline inference' --hybrid --limit 2
        if ($LASTEXITCODE -ne 0 -or ($searchOutput -join "`n") -notmatch 'second') {
            throw "Packaged offline hybrid search failed: $($searchOutput -join "`n")"
        }
    } finally {
        Pop-Location
    }
} finally {
    Remove-Item Env:GITSCRY_INSTALL_DIR -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
}
