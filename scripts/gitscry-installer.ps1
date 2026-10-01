$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$archiveName = 'gitscry-x86_64-pc-windows-msvc.zip'
$releaseBase = 'https://github.com/WodenJay/GitScry/releases/latest/download'
if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne [Runtime.InteropServices.Architecture]::X64) {
    throw 'GitScry release installers support Windows x86-64 only.'
}

$installDir = if ($env:GITSCRY_INSTALL_DIR) {
    $env:GITSCRY_INSTALL_DIR
} else {
    Join-Path $HOME '.gitscry\bin'
}
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) "gitscry-install-$([guid]::NewGuid().ToString('N'))"
$stageDir = $null
$backupPath = $null
$installCompleted = $false
New-Item -ItemType Directory -Path $tempRoot | Out-Null

try {
    $archivePath = Join-Path $tempRoot $archiveName
    $checksumPath = "$archivePath.sha256"
    Invoke-WebRequest -Uri "$releaseBase/$archiveName" -OutFile $archivePath
    Invoke-WebRequest -Uri "$releaseBase/$archiveName.sha256" -OutFile $checksumPath

    $checksumText = Get-Content -LiteralPath $checksumPath -Raw
    $checksumPattern = '(?m)^([0-9a-fA-F]{64})\s+\*?' + [regex]::Escape($archiveName) + '\s*$'
    $checksumMatch = [regex]::Match($checksumText, $checksumPattern)
    if (-not $checksumMatch.Success) {
        throw "The release checksum file does not contain a SHA-256 for $archiveName."
    }
    $expectedHash = $checksumMatch.Groups[1].Value.ToLowerInvariant()
    $actualHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualHash -ne $expectedHash) {
        throw "SHA-256 verification failed for $archiveName."
    }

    $packageDir = Join-Path $tempRoot 'package'
    Expand-Archive -LiteralPath $archivePath -DestinationPath $packageDir
    $runtimeSource = Join-Path $packageDir 'runtime'
    $executableSource = Join-Path $packageDir 'gitscry.exe'
    if (-not (Test-Path -LiteralPath $executableSource -PathType Leaf) -or
        -not (Test-Path -LiteralPath $runtimeSource -PathType Container)) {
        throw 'The verified GitScry archive is missing its executable or ONNX Runtime package.'
    }

    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    $stageDir = Join-Path $installDir ".gitscry-install-$([guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Path $stageDir | Out-Null
    $runtimeDestination = Join-Path $installDir 'runtime'
    New-Item -ItemType Directory -Path $runtimeDestination -Force | Out-Null
    Get-ChildItem -LiteralPath $runtimeSource -Force | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $runtimeDestination -Recurse -Force
    }

    $executablePath = Join-Path $installDir 'gitscry.exe'
    $stagedExecutable = Join-Path $stageDir 'gitscry.exe'
    Copy-Item -LiteralPath $executableSource -Destination $stagedExecutable
    if (Test-Path -LiteralPath $executablePath) {
        $backupPath = Join-Path $stageDir 'previous-gitscry.exe'
        Move-Item -LiteralPath $executablePath -Destination $backupPath
    }
    try {
        Move-Item -LiteralPath $stagedExecutable -Destination $executablePath
    } catch {
        if ($backupPath -and (Test-Path -LiteralPath $backupPath)) {
            Move-Item -LiteralPath $backupPath -Destination $executablePath
        }
        throw
    }
    $installCompleted = $true

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $pathEntries = @($userPath -split ';' | Where-Object { $_ })
    if (-not ($pathEntries | Where-Object { $_.TrimEnd('\') -ieq $installDir.TrimEnd('\') })) {
        $newUserPath = if ($userPath) { "$userPath;$installDir" } else { $installDir }
        try {
            [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
        } catch {
            Write-Warning "Could not add $installDir to the user PATH. Add it manually to run gitscry from any directory."
        }
    }
    Write-Output "GitScry installed to $executablePath. Open a new terminal to use gitscry."
} finally {
    if ($stageDir -and (Test-Path -LiteralPath $stageDir)) {
        if ($installCompleted -or -not $backupPath -or -not (Test-Path -LiteralPath $backupPath)) {
            Remove-Item -LiteralPath $stageDir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
    if ($tempRoot -and (Test-Path -LiteralPath $tempRoot)) {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
