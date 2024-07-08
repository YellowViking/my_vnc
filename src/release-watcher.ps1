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
                Stop-Process -Name "winvnc-server" -Force
                # start my-vnc.exe
                Expand-Archive -Path $path -DestinationPath C:\Users\User\Desktop -Force
                # set environment variable

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
            write-host "starting my-vnc.exe"
            Start-Sleep -Milliseconds 500
            $env:RUST_BACKTRACE = 1
            $env:RUST_LOG = "info"
            Invoke-Expression "C:\Users\User\Desktop\winvnc-server.exe --host fox-pc --port 80 --use-tunnelling"
        }
    }
}
finally
{
    Write-Host "Script has been stopped"
}
