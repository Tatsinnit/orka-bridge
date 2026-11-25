# Exercise real lifecycle predicates without launching Bridge or touching a user profile.
$ErrorActionPreference = 'Stop'
if (-not ('System.Windows.Automation.PropertyCondition' -as [type])) {
    Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
}
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'native-lifecycle-windows.ps1'), [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw "Lifecycle script parse errors: $errors" }
$trayMenu = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Tray-Menu' }, $true)
$trayPoint = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Find-BridgeTrayPoint' }, $true)
if (!$trayMenu -or !$trayPoint -or
    !$trayPoint.Extent.Text.Contains("Wait-For 'Native notification overflow did not close'") -or
    !$trayMenu.Extent.Text.Contains("Wait-For 'Open Bridge menu item missing'") -or
    !$trayMenu.Extent.Text.Contains("Wait-For 'Quit Bridge menu item missing'")) {
    throw 'Tray menu must reset a stale overflow and retain exact menu-item assertions'
}
& {
    $state = [pscustomobject]@{ Mode = 'retry'; Attempts = 0; Clicks = 0; Dismissals = 0 }
    function Find-BridgeTrayPoint([string]$Label) {
        $state.Attempts++
        if ($state.Mode -eq 'provider-error') { throw 'Native provider invariant failed' }
        [pscustomobject]@{ X = 100; Y = 200 }
    }
    function Open-TrayPopup($Point) { $state.Clicks++ }
    function Dismiss-NativePopup { $state.Dismissals++ }
    function Find-PopupControl([string]$Name) {
        if ($state.Mode -eq 'always-missing' -or ($state.Mode -eq 'retry' -and $state.Attempts -eq 1 -and $Name -ceq 'Open Bridge')) { return }
        "$Name/$($state.Attempts)"
    }
    function Wait-For([string]$Message, [scriptblock]$Predicate) {
        $value = & $Predicate
        if ($value) { return $value }
        throw $Message
    }
    . ([scriptblock]::Create($trayMenu.Extent.Text))

    $commands = @(Tray-Menu)
    if (($commands -join '|') -cne 'Open Bridge/2|Quit Bridge/2' -or $state.Attempts -ne 2 -or $state.Clicks -ne 2 -or $state.Dismissals -ne 1) {
        throw 'Tray menu did not retry from a freshly resolved icon and return the exact native commands'
    }

    $state.Mode = 'provider-error'; $state.Attempts = 0; $state.Clicks = 0; $state.Dismissals = 0
    $failure = try { Tray-Menu | Out-Null; $null } catch { $_.Exception.Message }
    if ($failure -cne 'Native provider invariant failed' -or $state.Attempts -ne 1) { throw 'Tray menu swallowed or retried an unrelated provider failure' }

    $state.Mode = 'always-missing'; $state.Attempts = 0; $state.Clicks = 0; $state.Dismissals = 0
    $failure = try { Tray-Menu | Out-Null; $null } catch { $_.Exception.Message }
    if ($failure -cne 'Open Bridge menu item missing' -or $state.Attempts -ne 3 -or $state.Clicks -ne 3 -or $state.Dismissals -ne 2) {
        throw 'Tray menu did not stop after three exact native-menu attempts'
    }
}
Write-Host 'PASS tray menu bounds native retries, returns exact items and preserves unrelated failures'
# Timestamp writes must stay off the success pipeline: callers return controls/handles.
foreach ($name in @('Write-StartupEvent', 'Test-NativeReady')) {
    $definition = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }, $true)
    if (!$definition) { throw "$name missing" }
    . ([scriptblock]::Create($definition.Extent.Text))
}
$script:themeOutput = Join-Path ([IO.Path]::GetTempPath()) "bridge-startup-test-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory $script:themeOutput | Out-Null
$script:startupLog = Join-Path $script:themeOutput 'app.err.log'
$script:app = [pscustomobject]@{ Id = 123 }
$script:hwnd = [IntPtr]456
try {
    $before = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $output = @(Write-StartupEvent 'frame-observed')
    $after = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $events = @(Get-Content (Join-Path $script:themeOutput 'startup-timeline.jsonl') | ConvertFrom-Json)
    if ($output.Count -ne 0 -or $events.Count -ne 1) { throw 'Timestamp event leaked output or was not written once' }
    $event = $events[0]
    if ($event.event -cne 'frame-observed' -or $event.epoch_ms -lt $before -or $event.epoch_ms -gt $after -or $event.hostPid -ne 123 -or $event.hwnd -ne 456) { throw 'Timestamp fields incorrect' }
    Write-Host 'PASS startup event correlates epoch, process and frame without success output'

    if (Test-NativeReady) { throw 'Missing stderr log was accepted as ready' }
    [IO.File]::WriteAllText($script:startupLog, "bridge-startup: main epoch_ms=1`nbridge-startup: installing lifecycle epoch_ms=2`n")
    if (Test-NativeReady) { throw 'Pre-ready milestones were accepted as ready' }
    [IO.File]::AppendAllText($script:startupLog, "bridge-startup: ready epoch_ms=3 elapsed_ms=2 pid=123`n")
    $ready = @(Test-NativeReady)
    if ($ready.Count -ne 1 -or $ready[0] -ne $true) { throw 'Timestamped readiness prefix no longer yields exactly one Boolean' }
    Write-Host 'PASS startup readiness still requires the existing ready marker with timestamp suffixes'

    # A missing destination makes the best-effort diagnostic write fail, not the walk.
    $script:themeOutput = Join-Path $script:themeOutput 'absent'
    $output = @(Write-StartupEvent 'frame-observed' -WarningAction SilentlyContinue)
    if ($output.Count) { throw 'Failed timestamp write leaked success output' }
    Write-Host 'PASS unavailable timeline destination does not fail startup or leak output'
} finally {
    Remove-Item -LiteralPath (Split-Path $script:startupLog) -Recurse -Force
}

$lookup = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Find-Action' }, $true)
if (!$lookup) { throw 'Find-Action missing' }
. ([scriptblock]::Create($lookup.Extent.Text))
$children = [System.Windows.Automation.TreeScope]::Descendants
$button = [System.Windows.Automation.ControlType]::Button
$root = [pscustomobject]@{ Candidates = @() }
$root | Add-Member ScriptMethod FindAll { param($scope, $condition) return $this.Candidates }

$cases = @(
    @{ Name = 'skip null before exact match'; Names = @($null, 'Set up'); Want = 1 }
    @{ Name = 'skip null before row action'; Names = @($null, 'Set up This computer'); Want = 1 }
    @{ Name = 'unnamed candidates never match'; Names = @($null, ''); Want = -1 }
    @{ Name = 'case and word boundary stay strict'; Names = @('set up', 'set up This computer', 'Set upper', 'Set upThis computer', "Set up`tThis computer", 'Other Set up'); Want = -1 }
    @{ Name = 'first matching candidate wins'; Names = @('Set up This computer', 'Set up'); Want = 0 }
)
foreach ($case in $cases) {
    $root.Candidates = @(foreach ($name in $case.Names) { [pscustomobject]@{ Current = [pscustomobject]@{ Name = $name } } })
    $actual = Find-Action $root 'Set up'
    if ($case.Want -lt 0) {
        if ($null -ne $actual) { throw "$($case.Name): unexpected match" }
    } elseif (![object]::ReferenceEquals($actual, $root.Candidates[$case.Want])) {
        throw "$($case.Name): wrong candidate"
    }
    Write-Host "PASS $($case.Name)"
}

$wait = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Wait-For' }, $true)
$savedWait = $ast.Find({
    param($node)
    $node -is [System.Management.Automation.Language.CommandAst] -and $node.GetCommandName() -eq 'Wait-For' -and
        $node.CommandElements[1].Value -eq 'Native editor change not saved'
}, $true)
if (!$wait -or !$savedWait) { throw 'Saved-file wait missing' }
. ([scriptblock]::Create($wait.Extent.Text))
$savedPredicate = $savedWait.CommandElements[2].ScriptBlock.GetScriptBlock()
$owned = [IO.Path]::GetTempFileName()
$script:marker = ' lifecycle-sharing-regression'
$writer = $null
try {
    $writer = [IO.File]::Open($owned, [IO.FileMode]::Open, [IO.FileAccess]::Write, [IO.FileShare]::None)
    $bytes = [Text.Encoding]::UTF8.GetBytes("instructions$($script:marker)")
    $writer.Write($bytes, 0, $bytes.Length)
    $writer.Flush()
    $attempts = [ref]0
    Wait-For 'Saved-file polling did not recover after the writer released' {
        $attempts.Value++
        try { & $savedPredicate }
        finally {
            # Keep the first real read locked; release before the next polling attempt.
            if ($attempts.Value -eq 1) { $writer.Dispose() }
        }
    } | Out-Null
    if ($attempts.Value -ne 2) { throw "Expected a locked read followed by success, got $($attempts.Value) attempts" }
    Write-Host 'PASS saved-file polling retries an exclusive writer then reads its content'

    [IO.File]::WriteAllText($owned, 'instructions without the marker')
    if (& $savedPredicate) { throw 'Saved-file polling accepted content without the marker' }
    Write-Host 'PASS saved-file polling still requires the marker'

    $readError = $null
    try {
        & {
            # Reproduce deletion between existence check and read, with real filesystem calls.
            function Test-Path($Path) {
                $exists = Microsoft.PowerShell.Management\Test-Path $Path
                if ($exists) { [IO.File]::Delete($Path) }
                return $exists
            }
            & $savedPredicate
        } | Out-Null
    } catch { $readError = $_.Exception.GetBaseException() }
    if ($readError -isnot [IO.FileNotFoundException] -or $readError.FileName -ne $owned) {
        throw "Saved-file polling masked or replaced an unexpected I/O error: $readError"
    }
    Write-Host 'PASS saved-file polling preserves non-sharing I/O errors and their path'
} finally {
    if ($writer) { $writer.Dispose() }
    Remove-Item -LiteralPath $owned -Force -ErrorAction SilentlyContinue
}
