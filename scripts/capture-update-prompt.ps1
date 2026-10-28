param(
    [string]$Exe = 'G:/Temp/agents/DLSS5/target/debug/Vivid.exe',
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '../docs/testing/update-prompt-2026-10-17')
)

$ErrorActionPreference = 'Stop'
$outputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null
$count = 0
foreach ($size in @('default', 'minimum')) {
    foreach ($theme in @('light', 'dark')) {
        foreach ($state in @('prompt', 'checking')) {
            $dimensions = if ($size -eq 'default') { '1060x720' } else { '860x540' }
            $flag = if ($state -eq 'prompt') { '--update-prompt' } else { '--update-checking' }
            $arguments = @('--screenshot', (Join-Path $outputDirectory "$size-$theme-$state.png"), '--size', $dimensions, '--theme', $theme, $flag)
            $process = Start-Process -FilePath $Exe -ArgumentList $arguments -WindowStyle Hidden -PassThru
            if (-not $process.WaitForExit(20000)) {
                $process.Kill()
                throw "Preview timeout: $size-$theme-$state"
            }
            if ($process.ExitCode -ne 0) { throw "Preview failed: $size-$theme-$state, exit $($process.ExitCode)" }
            $count += 1
        }
    }
}
Write-Output "Captured $count native A-tier update previews in $outputDirectory"
