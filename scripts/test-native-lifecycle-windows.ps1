# Exercise the real lookup without launching Bridge or touching a user profile.
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
