param(
    [string]$Exe = 'G:/Temp/agents/DLSS5/target/debug/Vivid.exe',
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '../docs/testing/about-2026-10-17')
)

$ErrorActionPreference = 'Stop'
$outputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null
$cases = @()
foreach ($scale in @('1', '1.25', '1.5')) {
    foreach ($windowSize in @('default', 'minimum')) {
        foreach ($theme in @('light', 'dark')) {
            $suffix = if ($scale -eq '1') { '' } else { '-' + $scale }
            $cases += @{ Name = "$windowSize-$theme$suffix"; Size = $(if ($windowSize -eq 'default') { '1060x720' } else { '860x540' }); Theme = $theme; Scale = $scale; Extra = @() }
        }
    }
}
$cases += @{ Name = 'coffee'; Size = '1060x720'; Theme = 'light'; Scale = '1'; Extra = @('--coffee') }
$cases += @{ Name = 'coffee-dark'; Size = '1060x720'; Theme = 'dark'; Scale = '1'; Extra = @('--coffee') }
$cases += @{ Name = 'zoom'; Size = '1060x720'; Theme = 'dark'; Scale = '1'; Extra = @('--zoom') }
$cases += @{ Name = 'minimum-coffee'; Size = '860x540'; Theme = 'light'; Scale = '1'; Extra = @('--coffee') }
$cases += @{ Name = 'minimum-zoom'; Size = '860x540'; Theme = 'dark'; Scale = '1'; Extra = @('--zoom') }
foreach ($case in $cases) {
    $arguments = @('--screenshot', (Join-Path $outputDirectory ($case.Name + '.png')), '--size', $case.Size, '--theme', $case.Theme, '--scale', $case.Scale) + $case.Extra
    $process = Start-Process -FilePath $Exe -ArgumentList $arguments -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit(20000)) {
        $process.Kill()
        throw "Preview timeout: $($case.Name)"
    }
    if ($process.ExitCode -ne 0) { throw "Preview failed: $($case.Name), exit $($process.ExitCode)" }
}
Write-Output "Captured $($cases.Count) native previews in $outputDirectory"
