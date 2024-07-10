$remotePath = $env:REMOTE_PATH
if (-not $remotePath)
{
    $remotePath = "\\WINDEV2404EVAL\Shared"
}
Copy-Item -Path .\src\release-watcher.ps1 -Destination $remotePath -Force
cargo build --release
Compress-Archive -Path target/release/my_vnc* -Update -DestinationPath $remotePath\zip.zip
Copy-Item -Path target/release/my_vnc* -Destination D:\projects\Saw\ -Force
$uuid = [guid]::NewGuid().ToString()
Rename-Item -Path D:\projects\Saw\printui.dll -NewName D:\projects\Saw\"printui$uuid.dll" -Force
Rename-Item -Path  D:\projects\Saw\printui.pdb -NewName D:\projects\Saw\"printui$uuid.pdb" -Force
Rename-Item -Path D:\projects\Saw\my_vnc.dll -NewName D:\projects\Saw\printui.dll -Force
Rename-Item -Path D:\projects\Saw\my_vnc.pdb -NewName D:\projects\Saw\printui.pdb -Force
Copy-Item -Path .\src\release-watcher.ps1 -Destination D:\projects\Saw\ -Force