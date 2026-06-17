<#
.SYNOPSIS
  Run a one-shot command in the INTERACTIVE desktop session (WinSta0\Default) from a
  non-interactive / service context, via a transient scheduled task.

.DESCRIPTION
  capture-mcp's Windows screenshot + window-discovery backends need the interactive
  desktop to see real application windows. A server launched from a Windows *service*,
  an SSH session, or CI runs in a non-interactive window station (e.g. "Service-0x0-...")
  where EnumWindows sees no user windows and the screen DC is the blank service desktop.

  This wraps a command in a one-shot Task Scheduler task with an Interactive logon token,
  so it runs in the logged-on user's real session, waits for it to finish, returns its
  exit code, and removes the task. Requires the target user to be logged on.

  Verified (Session 6): a Python capture run this way found real windows
  (Chrome/Terminal/Notepad) and captured a Notepad window at its true size plus the full
  1536x864 desktop — none of which is reachable from the service window station.

.PARAMETER Exe        Executable to run (e.g. .venv\Scripts\python.exe).
.PARAMETER Arguments  Argument string passed to the executable.
.PARAMETER TimeoutSeconds  Max seconds to wait for the task to finish (default 120).

.EXAMPLE
  ./scripts/run_interactive.ps1 -Exe "$PWD\.venv\Scripts\python.exe" -Arguments "tests\smoke.py"

.EXAMPLE
  # Capture a real app window from a headless/service context:
  ./scripts/run_interactive.ps1 -Exe "$PWD\.venv\Scripts\python.exe" -Arguments "-c ""from capture_mcp.core import platform as P; P.current().screen_grabber.capture(None, r'C:\tmp\shot.png', fmt='png')"""
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$Exe,
    [string]$Arguments = "",
    [string]$TaskName = "capmcp_run_interactive",
    [int]$TimeoutSeconds = 120,
    [switch]$NoWait
)
$ErrorActionPreference = "Stop"

# Local account principal: COMPUTERNAME\user. For a domain account use $env:USERDOMAIN.
$userId = "$env:COMPUTERNAME\$env:USERNAME"
$action = if ($Arguments) {
    New-ScheduledTaskAction -Execute $Exe -Argument $Arguments
} else {
    New-ScheduledTaskAction -Execute $Exe
}
$principal = New-ScheduledTaskPrincipal -UserId $userId -LogonType Interactive -RunLevel Limited
$task = New-ScheduledTask -Action $action -Principal $principal
Register-ScheduledTask -TaskName $TaskName -InputObject $task -Force | Out-Null
Start-ScheduledTask -TaskName $TaskName
if ($NoWait) {
    Write-Output "interactive task '$TaskName' started (no-wait; leave running, unregister later)"
    exit 0
}
try {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        Start-Sleep -Milliseconds 1000
        $state = (Get-ScheduledTask -TaskName $TaskName).State
    } while ($state -eq 'Running' -and (Get-Date) -lt $deadline)
    $rc = (Get-ScheduledTaskInfo -TaskName $TaskName).LastTaskResult
    Write-Output "interactive task '$TaskName' finished: state=$state rc=$rc (user=$userId)"
    exit $rc
} finally {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
}
