param (
    [string]$Url
)

function Find-BuildInfo {
    [CmdletBinding()]
    param (
        [Parameter(Mandatory=$true, ValueFromPipeline=$true, Position=0)]
        [string]$Path
    )

    process {
        try {
            $fileContent = Get-Content -Path $Path -ErrorAction Stop
            $found = $false

            $regex = '(Master|Release|PR|Local) Build.+(?:cef_)?(\d+\.\d+\.\d+\+g[0-9a-f]+\+chromium-\d+\.\d+\.\d+\.\d+)'

            foreach ($line in $fileContent) {
                $match = [regex]::Match($line, $regex)
                if ($match.Success) {
                    $buildType = $match.Groups[1].Value
                    Write-Output "Тип билда: $buildType"
                    $found = $true
                    # Записываем вывод в файл окружения
                    "build_type=$buildType" | Out-File -FilePath $env:GITHUB_OUTPUT -Encoding utf8 -Append
                    # Прерываем цикл после первого найденного совпадения
                    break
                }
            }

            if (-not $found) {
                Write-Output "Билд не найден"
                "build_type=false" | Out-File -FilePath $env:GITHUB_OUTPUT -Encoding utf8 -Append
            }
        }
        catch {
            Write-Error "Ошибка при чтении файла: $_"
            "build_type=false" | Out-File -FilePath $env:GITHUB_OUTPUT -Encoding utf8 -Append
        }
    }
}

function Download-And-Unpack-Spotify {
    [CmdletBinding()]
    # Параметр $Url теперь передается из области видимости скрипта
    
    $tempPath = Join-Path ([System.IO.Path]::GetTempPath()) "spotify"
    $destinationPath = Join-Path $tempPath "unpacked"
    $exePath = Join-Path $tempPath "SpotifySetup.exe"

    if (-not (Test-Path -Path $tempPath)) {
        New-Item -ItemType Directory -Path $tempPath | Out-Null
    }
    if (-not (Test-Path -Path $destinationPath)) {
        New-Item -ItemType Directory -Path $destinationPath | Out-Null
    }

    try {
        Write-Output "Скачивание файла из $Url..."
        Invoke-WebRequest -Uri $Url -OutFile $exePath
        Write-Output "Файл сохранен в $exePath"

        Write-Output "Распаковка файла в $destinationPath..."
        Start-Process -Wait -FilePath $exePath -ArgumentList "/extract `"$destinationPath`""
        Write-Output "Распаковка завершена"

        $dllPath = Join-Path $destinationPath "Spotify.dll"
        $exePathForAnalysis = Join-Path $destinationPath "Spotify.exe"

        if (Test-Path $dllPath) {
            Find-BuildInfo -Path $dllPath
        } elseif (Test-Path $exePathForAnalysis) {
            Find-BuildInfo -Path $exePathForAnalysis
        }
        else {
            Write-Error "Файлы Spotify.dll и Spotify.exe не найдены в $destinationPath"
            "build_type=false" | Out-File -FilePath $env:GITHUB_OUTPUT -Encoding utf8 -Append
        }
    }
    catch {
        Write-Error "Произошла ошибка: $_"
        "build_type=false" | Out-File -FilePath $env:GITHUB_OUTPUT -Encoding utf8 -Append
    }
}


if (-not [string]::IsNullOrEmpty($Url)) {
    Download-And-Unpack-Spotify
} else {
    Write-Error "URL не был предоставлен."
    "build_type=false" | Out-File -FilePath $env:GITHUB_OUTPUT -Encoding utf8 -Append
}