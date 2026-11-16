# Exercise real lifecycle predicates without launching Bridge or touching a user profile.
$ErrorActionPreference = 'Stop'
if (-not ('System.Windows.Automation.PropertyCondition' -as [type])) {
    Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
}
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'native-lifecycle-windows.ps1'), [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw "Lifecycle script parse errors: $errors" }
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
