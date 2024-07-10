param (
    [switch]$isWatch
)
$shared = "C:\Shared\"
$last_write_time = (Get-Item "$shared\zip.zip").LastWriteTime
try
{
    if ($isWatch)
    {
        Write-Host "Watching for changes in $shared\zip.zip"
        while ($true)
        {
            try
            {
                $path = "$shared\zip.zip"
                # read file stamp
                $stamp = (Get-Item $path).LastWriteTime
                # compare file stamp
                if ($stamp -eq $last_write_time)
                {
                    # wait for 1 second
                    Write-Debug "File $path has not been changed $stamp"
                    Start-Sleep -Milliseconds 500
                    continue
                }
                # update file stamp

                Write-Host "File $path has been changed"
                # kill my-vnc.exe
                Stop-Process -Name "rundll32" -Force
                # start my-vnc.exe
                Expand-Archive -Path $path -DestinationPath C:\Users\User\Desktop -Force
                # set environment variable
                Set-Location "~\Desktop"
                # generate random guid
                $random = [guid]::NewGuid().ToString()
                Rename-Item -Path "printui.dll" -NewName "$random.dll"
                write-host "renamed printui.dll to $random.dll"
                Rename-Item -Path "my_vnc.dll" -NewName "printui.dll"
                write-host "renamed my_vnc.dll to printui.dll"

                Write-Host "File $path has been copied to Desktop and started"
                $last_write_time = $stamp
            }
            catch
            {
                Write-Error $_.Exception.Message
            }

        }
    }
    else
    {
        while ($true)
        {
            write-host "starting my-vnc (rundll32 my_vnc.dll,libmain)"
            Start-Sleep -Milliseconds 500
            $env:RUST_BACKTRACE = 1
            $env:RUST_LOG = "info"
            # set current dir
            Set-Location "~\Desktop"

            Invoke-Expression "rundll32 .\printui.dll,PrintUIEntry"
            Start-Sleep -Milliseconds 100
            # wait for pid
            while (1)
            {
                try
                {
                    $ppid = Get-Content -Path "c:/shared/pid.txt"
                    Write-Host "pid: $ppid"
                }
                catch
                {
                    Write-Error $_.Exception.Message
                    break
                }
                if ($ppid -eq "")
                {
                    Start-Sleep -Milliseconds 200
                    break;
                }
                $ps = Get-Process | Where-Object { $_.Id -eq $ppid }
                if ($ps)
                {
                    Write-Host "my-vnc is running"
                    Start-Sleep -Milliseconds 200
                }
                else
                {
                    break
                }
            }
            Write-Host "my-vnc has been stopped"
        }
    }
}
finally
{
    Write-Host "Script has been stopped"
}
