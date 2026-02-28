import re
import json
import requests
from packaging import version
import sys
from datetime import datetime
import os
import subprocess
import tempfile
from requests.exceptions import RequestException
import time

VERSIONS_JSON_FILE = "versions_deb.json"
SPOTIFY_REPO_URL = (
    "https://repository-origin.spotify.com/pool/non-free/s/spotify-client/"
)
REPO_UPLOAD = "LoaderSpot/deb-builds"


def get_spotify_versions_from_repo():
    urls = [
        "https://repository.spotify.com/dists/testing/non-free/binary-amd64/Packages",
        SPOTIFY_REPO_URL,
    ]

    version_pattern = r"(\d+\.\d+\.\d+\.\d+\.g[0-9a-f]{8})"
    versions = set()

    for url in urls:
        try:
            response = requests.get(url)
            response.raise_for_status()
            found_versions = re.findall(version_pattern, response.text)
            versions.update(found_versions)
        except requests.exceptions.RequestException as e:
            print(f"Ошибка при запросе {url}: {e}")

    min_version = version.parse("1.1.58")
    return [
        v for v in versions if version.parse(".".join(v.split(".")[:3])) > min_version
    ]


def get_existing_versions():
    try:
        with open(VERSIONS_JSON_FILE, "r", encoding="utf-8") as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError) as e:
        print(f"Ошибка при чтении файла {VERSIONS_JSON_FILE}: {e}")
        return {}


def parse_version_key(full_version):
    match = re.match(r"(\d+\.\d+\.\d+\.\d+)\..*", full_version)
    return match.group(1) if match else None


def find_new_versions(repo_versions, existing_data):
    new_versions = []

    for full_version in repo_versions:
        version_key = parse_version_key(full_version)
        if version_key and version_key not in existing_data:
            new_versions.append((version_key, full_version))

    return new_versions


def get_file_info(full_version):
    url = f"{SPOTIFY_REPO_URL}spotify-client_{full_version}_amd64.deb"

    try:
        response = requests.head(url, timeout=10)
        if response.status_code == 200:
            size = response.headers.get("Content-Length", "0")

            last_modified = response.headers.get("Last-Modified", "")
            if last_modified:
                date_obj = datetime.strptime(last_modified, "%a, %d %b %Y %H:%M:%S %Z")
                formatted_date = date_obj.strftime("%d.%m.%Y")
            else:
                formatted_date = ""

            return {"size": size, "data": formatted_date}
        else:
            print(f"Ошибка HEAD запроса к {url}: {response.status_code}")
    except Exception as e:
        print(f"Ошибка при запросе информации о файле: {e}")

    return {"size": "0", "data": ""}


def sort_versions(versions_dict):
    def version_key(item):
        return tuple(map(int, item[0].split(".")))

    return dict(sorted(versions_dict.items(), key=version_key, reverse=True))


def update_json_file(sorted_data):
    try:
        with open(VERSIONS_JSON_FILE, "w", encoding="utf-8") as f:
            json.dump(sorted_data, f, indent=2)
        print(f"Файл {VERSIONS_JSON_FILE} успешно обновлен")
        return True
    except Exception as e:
        print(f"Ошибка при обновлении файла {VERSIONS_JSON_FILE}: {e}")
        return False


def commit_changes(new_versions):
    try:
        github_actions = os.environ.get("GITHUB_ACTIONS") == "true"

        if github_actions:
            subprocess.run(["git", "config", "--global", "user.name", "GitHub Actions"])
            subprocess.run(
                ["git", "config", "--global", "user.email", "actions@github.com"]
            )

        subprocess.run(["git", "add", VERSIONS_JSON_FILE])

        # Формируем сообщение коммита в зависимости от количества новых версий
        if new_versions:
            commit_message = (
                f"Added deb version {new_versions[0][1]}"
                if len(new_versions) == 1
                else f"Added deb versions: {', '.join(v[1] for v in new_versions)}"
            )
        else:
            commit_message = "Update Spotify versions"

        # Создаем коммит
        subprocess.run(["git", "commit", "-m", commit_message])

        if github_actions:
            token = get_github_token()
            if token:
                repo_url = f"https://{token}@github.com/{os.environ.get('GITHUB_REPOSITORY')}.git"
                subprocess.run(["git", "push", repo_url, "HEAD:main"])
            else:
                subprocess.run(["git", "push"])
        else:
            subprocess.run(["git", "push"])

        print("Изменения успешно отправлены в репозиторий")
        return True
    except Exception as e:
        print(f"Ошибка при отправке изменений в репозиторий: {e}")
        return False


def download_deb_file(full_version):
    url = f"{SPOTIFY_REPO_URL}spotify-client_{full_version}_amd64.deb"
    filename = f"spotify-client_{full_version}_amd64.deb"

    try:
        print(f"Скачивание {filename}...")
        with tempfile.NamedTemporaryFile(delete=False, suffix=".deb") as temp_file:
            response = requests.get(url, stream=True, timeout=60)
            response.raise_for_status()

            for chunk in response.iter_content(chunk_size=8192):
                if chunk:
                    temp_file.write(chunk)

            return temp_file.name, filename
    except RequestException as e:
        print(f"Ошибка при скачивании файла: {e}")
        return None, None


def get_github_token():
    token = os.environ.get("PERSONAL_TOKEN")
    if not token:
        print("ОШИБКА: Не найден PERSONAL_TOKEN в переменных окружения")
    return token


def get_latest_release(token):
    headers = {
        "Authorization": f"token {token}",
        "Accept": "application/vnd.github.v3+json",
    }

    url = f"https://api.github.com/repos/{REPO_UPLOAD}/releases/latest"

    try:
        response = requests.get(url, headers=headers)
        response.raise_for_status()
        return response.json()
    except RequestException as e:
        print(f"Ошибка при получении последнего релиза: {e}")
        return None


def upload_file_to_release(token, release_id, file_path, file_name):
    headers = {
        "Authorization": f"token {token}",
        "Accept": "application/vnd.github.v3+json",
        "Content-Type": "application/octet-stream",
    }

    url = f"https://uploads.github.com/repos/{REPO_UPLOAD}/releases/{release_id}/assets?name={file_name}"

    try:
        with open(file_path, "rb") as file:
            print(f"Загрузка {file_name} в релиз...")
            response = requests.post(url, headers=headers, data=file)
            response.raise_for_status()
            print(f"Файл {file_name} успешно загружен в релиз")
            return True
    except RequestException as e:
        print(f"Ошибка при загрузке файла в релиз: {e}")
        return False
    finally:
        # Удаляем временный файл
        try:
            os.remove(file_path)
        except Exception as e:
            print(f"Не удалось удалить временный файл {file_path}: {e}")


def main():
    # Получаем версии из репозитория Spotify
    spotify_versions = get_spotify_versions_from_repo()

    # Получаем существующие версии из JSON
    existing_data = get_existing_versions()

    # Находим новые версии
    new_versions = find_new_versions(spotify_versions, existing_data)

    if not new_versions:
        print("Новых версий не найдено")
        return False

    print(f"Найдено новых версий: {len(new_versions)}")

    # Если есть новые версии, получаем токен GitHub и загружаем файлы
    github_token = get_github_token()
    if github_token:
        latest_release = get_latest_release(github_token)
        if latest_release:
            release_id = latest_release["id"]

            # Скачиваем и загружаем каждую новую версию
            for version_key, full_version in new_versions:
                temp_file_path, file_name = download_deb_file(full_version)
                if temp_file_path:
                    time.sleep(2)
                    upload_file_to_release(
                        github_token, release_id, temp_file_path, file_name
                    )

    # Добавляем новые версии в существующие данные
    for version_key, full_version in new_versions:
        file_info = get_file_info(full_version)

        existing_data[version_key] = {
            "fullversion": full_version,
            "size": file_info["size"],
            "data": file_info["data"],
            "links": {
                "amd64": f"https://github.com/LoaderSpot/deb-builds/releases/download/builds/spotify-client_{full_version}_amd64.deb"
            },
        }

    # Сортируем и обновляем данные
    sorted_data = sort_versions(existing_data)
    if update_json_file(sorted_data):
        # Если мы находимся в GitHub Actions, выполняем коммит
        if os.environ.get("GITHUB_ACTIONS") == "true":
            commit_changes(new_versions)
        print("Обновление выполнено успешно")

    return True


if __name__ == "__main__":
    main()
    sys.exit(0)
