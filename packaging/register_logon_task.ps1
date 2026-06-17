<#
.SYNOPSIS
  Register / unregister the Capture tray agent as an INTERACTIVE LOGON TASK.

.DESCRIPTION
  capture-mcp's Windows daemon must run in the interactive WinSta0 desktop (window discovery +
  screenshots + the GPU/DirectX GUI renderer need it), so it is started by a Task Scheduler
  *logon task* — NEVER a Windows Service (a service runs in a non-interactive station). The task
  launches the tray agent (Capture.exe), which in turn spawns the daemon and owns its lifecycle.
  This is the Windows analogue of macOS launchd → CaptureBar. See docs/specs/windows-release.md.

  The Inno Setup installer calls this at install/uninstall time; it also works from source.

.PARAMETER Action     register (default) | unregister
.PARAMETER AgentPath  Path to Capture.exe (the installer passes the installed location).
.PARAMETER TaskName   Scheduled-task name (default 'CaptureAgent').
#>
[CmdletBinding()]
param(
    [ValidateSet('register', 'unregister')] [string]$Action = 'register',
    [string]$AgentPath = "$PSScriptRoot\..\gui\target\release\Capture.exe",
    [string]$TaskName = 'CaptureAgent'
)
$ErrorActionPreference = 'Stop'

if ($Action -eq 'unregister') {
    try {
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction Stop
        Write-Output "unregistered '$TaskName'"
    } catch {
        Write-Output "no task '$TaskName' to remove"
    }
    return
}

if (-not (Test-Path $AgentPath)) { throw "agent not found: $AgentPath" }
$AgentPath = (Resolve-Path $AgentPath).Path
$userId = "$env:COMPUTERNAME\$env:USERNAME"
# NB: avoid a local named $action — PowerShell vars are case-insensitive, so it would clobber
# the $Action parameter.
$taskAction = New-ScheduledTaskAction -Execute $AgentPath
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $userId
# Interactive logon token (WinSta0) at Limited run level — no admin/UAC needed for a self task.
$principal = New-ScheduledTaskPrincipal -UserId $userId -LogonType Interactive -RunLevel Limited
# Long-lived: no execution time limit, survive battery transitions.
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit ([TimeSpan]::Zero)
$task = New-ScheduledTask -Action $taskAction -Trigger $trigger -Principal $principal -Settings $settings
Register-ScheduledTask -TaskName $TaskName -InputObject $task -Force | Out-Null
Write-Output "registered '$TaskName' -> $AgentPath (runs at logon, interactive desktop)"
