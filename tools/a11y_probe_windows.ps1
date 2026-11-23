# Read the accessibility tree of a running `rustic view` through UI Automation.
#
# The M8 `a11y.acceptance` probe for Windows (ADR 0073): the same check as
# tools/a11y_probe_linux.py, made through the UIAutomationClient that
# Narrator and NVDA's UIA support use. Usage:
#
#     powershell -File tools/a11y_probe_windows.ps1 target\debug\rustic.exe
#
# Exit status 0 means every expectation held; the tree it read is printed
# either way.
param([Parameter(Mandatory = $true)][string]$Rustic)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$fixture = (Resolve-Path (Join-Path $PSScriptRoot '..\tests\a11y\probe.html')).Path
$expected = @(
    @('ControlType.Document', 'Accessibility probe'),
    @('ControlType.Hyperlink', 'First section'),
    @('ControlType.Button', 'Send'),
    @('ControlType.CheckBox', 'Remember me'),
    @('ControlType.Image', 'Company logo')
)

function Get-Tree([System.Windows.Automation.AutomationElement]$root) {
    $walker = [System.Windows.Automation.TreeWalker]::RawViewWalker
    $nodes = New-Object System.Collections.Generic.List[object]
    $stack = New-Object System.Collections.Generic.Stack[object]
    $stack.Push(@($root, 0))
    while ($stack.Count -gt 0 -and $nodes.Count -lt 2000) {
        $entry = $stack.Pop()
        $element = $entry[0]
        try {
            $current = $element.Current
            $toggle = $null
            $pattern = $null
            if ($element.TryGetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern, [ref]$pattern)) {
                $toggle = $pattern.Current.ToggleState.ToString()
            }
            $nodes.Add([pscustomobject]@{
                Depth = $entry[1]; Type = $current.ControlType.ProgrammaticName; Name = $current.Name
                Focusable = $current.IsKeyboardFocusable; Toggle = $toggle
                Width = $current.BoundingRectangle.Width; Height = $current.BoundingRectangle.Height
            })
        } catch {
            continue
        }
        $children = New-Object System.Collections.Generic.List[object]
        $child = $walker.GetFirstChild($element)
        while ($null -ne $child) {
            $children.Add($child)
            $child = $walker.GetNextSibling($child)
        }
        for ($i = $children.Count - 1; $i -ge 0; $i--) {
            $stack.Push(@($children[$i], ($entry[1] + 1)))
        }
    }
    return $nodes
}

function Test-Found($nodes, $type, $name) {
    return @($nodes | Where-Object { $_.Type -eq $type -and $_.Name -eq $name }).Count -gt 0
}

$process = Start-Process -FilePath $Rustic -ArgumentList @('view', '--window', '800x600', $fixture) -PassThru
try {
    $nodes = @()
    $deadline = (Get-Date).AddSeconds(90)
    while ((Get-Date) -lt $deadline) {
        if ($process.HasExited) {
            Write-Error "rustic view exited early with $($process.ExitCode)"
            exit 1
        }
        $condition = New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::ProcessIdProperty, $process.Id)
        $window = [System.Windows.Automation.AutomationElement]::RootElement.FindFirst(
            [System.Windows.Automation.TreeScope]::Children, $condition)
        if ($null -ne $window) {
            $nodes = Get-Tree $window
            $missing = @($expected | Where-Object { -not (Test-Found $nodes $_[0] $_[1]) })
            if ($missing.Count -eq 0) { break }
        }
        Start-Sleep -Milliseconds 500
    }
    foreach ($node in $nodes) {
        $line = ('  ' * $node.Depth) + $node.Type
        if ($node.Name) { $line += " `"$($node.Name)`"" }
        if ($node.Focusable) { $line += ' focusable' }
        if ($node.Toggle) { $line += " toggle=$($node.Toggle)" }
        $line += " $([int]$node.Width)x$([int]$node.Height)"
        Write-Output $line
    }
    $missing = @($expected | Where-Object { -not (Test-Found $nodes $_[0] $_[1]) })
    if ($missing.Count -gt 0) {
        Write-Output ("missing from the UI Automation tree: " + (($missing | ForEach-Object { "$($_[0]) '$($_[1])'" }) -join ', '))
        exit 1
    }
    foreach ($name in @('Send', 'First section', 'Remember me')) {
        if (@($nodes | Where-Object { $_.Name -eq $name -and $_.Focusable }).Count -eq 0) {
            Write-Output "'$name' must be keyboard focusable"
            exit 1
        }
    }
    if (@($nodes | Where-Object { $_.Type -eq 'ControlType.CheckBox' -and $_.Toggle -eq 'On' }).Count -eq 0) {
        Write-Output 'the checkbox must report ToggleState On'
        exit 1
    }
    Write-Output 'UI Automation probe passed'
    exit 0
} finally {
    if (-not $process.HasExited) { Stop-Process -Id $process.Id -Force }
}
