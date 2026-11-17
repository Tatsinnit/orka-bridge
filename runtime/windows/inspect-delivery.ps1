# Inspect the actual app ZIP using Windows-native extraction; never boot a VM.
param([Parameter(Mandatory)][string]$Archive)
$ErrorActionPreference = 'Stop'
$policy = Get-Content (Join-Path $PSScriptRoot 'runtime-inputs.json') -Raw | ConvertFrom-Json
$clean = Join-Path $env:RUNNER_TEMP ('Bridge delivery café ' + [guid]::NewGuid())
Expand-Archive -LiteralPath $Archive -DestinationPath $clean
$qemuRoot = Join-Path $clean 'native/qemu'
$manifestPath = Join-Path $qemuRoot 'provenance/runtime-manifest.json'
if ((Get-Item $manifestPath).Length -ne $policy.runtimeManifest.bytes -or
    (Get-FileHash $manifestPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $policy.runtimeManifest.sha256) {
    throw 'Extracted runtime manifest differs from native pin'
}
$manifest = Get-Content $manifestPath -Raw | ConvertFrom-Json
$expected = @('provenance/runtime-manifest.json') + @($manifest.files.path)
$actual = @(Get-ChildItem $qemuRoot -File -Recurse | ForEach-Object {
    [IO.Path]::GetRelativePath($qemuRoot, $_.FullName).Replace('\', '/')
})
if (Compare-Object ($expected | Sort-Object) ($actual | Sort-Object)) { throw 'Extracted runtime inventory differs' }
foreach ($row in $manifest.files) {
    $path = Join-Path $qemuRoot $row.path
    if ((Get-Item $path).Length -ne $row.bytes -or
        (Get-FileHash $path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $row.sha256) {
        throw "Extracted runtime file differs: $($row.path)"
    }
}
if (!(Test-Path (Join-Path $clean 'orka-bridge.exe')) -or !(Test-Path (Join-Path $clean 'QEMU-SOURCE.txt')) -or
    (Get-ChildItem $clean -Recurse -Filter 'bridge-windows-runtime-source.zip')) { throw 'App/source layout differs' }
$priorPath = $env:PATH
Push-Location $clean
try {
    $env:PATH = "$env:SystemRoot;$env:SystemRoot\System32"
    $qemu = Join-Path $qemuRoot 'bin/qemu-system-x86_64.exe'
    $img = Join-Path $qemuRoot 'bin/qemu-img.exe'
    $version = & $qemu --version
    if ($LASTEXITCODE -ne 0 -or ($version -join "`n") -notmatch '\b11\.0\.0\b') { throw 'QEMU version differs' }
    $machines = & $qemu -machine help
    if ($LASTEXITCODE -ne 0 -or ($machines -join "`n") -notmatch '\bpc-q35-11\.0\b') { throw 'Pinned q35 missing' }
    $accels = & $qemu -accel help
    if ($LASTEXITCODE -ne 0 -or ($accels -join "`n") -notmatch '(?im)^\s*whpx\b' -or
        ($accels -join "`n") -match '(?im)^\s*tcg\b') { throw 'Accelerator profile differs' }
    $version = & $img --version
    if ($LASTEXITCODE -ne 0 -or ($version -join "`n") -notmatch '\b11\.0\.0\b') { throw 'qemu-img version differs' }
} finally {
    $env:PATH = $priorPath
    Pop-Location
}
Write-Output "Verified $($actual.Count) runtime files in the portable app; no VM boot."
