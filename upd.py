import aiohttp
import asyncio
import json
import re
import argparse
from bs4 import BeautifulSoup
import urllib.parse

parser = argparse.ArgumentParser()
parser.add_argument("-v", required=True)
parser.add_argument("-s", required=True)
parser.add_argument("-u", required=True)

args = parser.parse_args()
version = args.v
source = args.s
u = args.u


async def send_request(json_data):
    url = u + urllib.parse.quote(json_data)
    async with aiohttp.ClientSession() as session:
        async with session.get(url) as response:
            if response.status == 200:
                data = await response.text()
                if "<div" in data:
                    soup = BeautifulSoup(data, "html.parser")
                    div_element = soup.find(
                        "div",
                        style="text-align:center;font-family:monospace;margin:50px auto 0;max-width:600px",
                    )
                    if div_element:
                        system_response = div_element.text
                    else:
                        system_response = "Не удалось извлечь ответ из HTML"
                else:
                    system_response = data.strip()
                
                print(f"Ответ от GAS: {system_response}")


async def check_url(session, url, platform_name):
    try:
        async with session.get(url) as response:
            if response.status == 200:
                if platform_name:
                    return response.url, platform_name
                else:
                    return response.url
    except aiohttp.ClientError:
        pass
    return None


def parse_version(ver_str):
    parts = ver_str.split(".")[:4]
    return tuple(int(x) for x in parts)


async def pre_version(latest_urls):
    message = f'Версия {version} {"отправлена" if latest_urls else "не найдена"}'
    print(message)

    if not latest_urls:
        latest_urls = {"unknown": "unknown", "version": version}

    latest_urls["version"] = version
    latest_urls["source"] = source

    req_ver = json.dumps(latest_urls, ensure_ascii=False, indent=2)
    await send_request(req_ver)


def get_urls(find_url):
    platform_urls = {}
    version_pattern = re.compile(r"-([0-9]+)\.(exe|tbz)")

    for url, platform in find_url:
        match = version_pattern.search(str(url))
        if match:
            version_number = int(match.group(1))
            if platform in platform_urls:
                # Если найдено больше одного url для платформы, то выбираем с самой большой ревизией
                current_version = int(
                    version_pattern.search(platform_urls[platform]).group(1)
                )
                if version_number > current_version:
                    platform_urls[platform] = str(url)
            else:
                platform_urls[platform] = str(url)

    return platform_urls


async def main():
    start_number = 0
    before_enter = 1000
    additional_searches = 10  # Максимальное кол-во шагов для дополнительного поиска
    increment = 1000  # Размер шага

    find_url = []

    root_url = "https://upgrade.scdn.co/upgrade/client/"
    platform_templates = {
        "WIN32": "win32-x86/spotify_installer-{version}-{numbers}.exe",
        "WIN64": "win32-x86_64/spotify_installer-{version}-{numbers}.exe",
        "WIN-ARM64": "win32-arm64/spotify_installer-{version}-{numbers}.exe",
        "OSX": "osx-x86_64/spotify-autoupdate-{version}-{numbers}.tbz",
        "OSX-ARM64": "osx-arm64/spotify-autoupdate-{version}-{numbers}.tbz",
    }

    # не ищем архитектуру WIN32 если версия >= 1.2.54.304
    if parse_version(version) >= (1, 2, 54, 304):
        platform_templates.pop("WIN32", None)

    async with aiohttp.ClientSession() as session:
        tasks = []
        platform_names = list(platform_templates.keys())

        for platform_name in platform_names:
            numbers = start_number
            while numbers <= before_enter:
                url = root_url + platform_templates[platform_name].format(
                    version=version, numbers=numbers
                )
                tasks.append(check_url(session, url, platform_name))
                numbers += 1

        for task in asyncio.as_completed(tasks):
            result = await task
            if result is not None:
                find_url.append(result)

        for i in range(additional_searches):
            latest_urls = get_urls(find_url)
            if len(latest_urls) < len(platform_names):
                start_number = before_enter + 1
                before_enter += increment
                tasks = []
                for platform_name in platform_names:
                    if platform_name not in latest_urls:
                        numbers = start_number
                        while numbers <= before_enter:
                            url = root_url + platform_templates[platform_name].format(
                                version=version, numbers=numbers
                            )
                            tasks.append(check_url(session, url, platform_name))
                            numbers += 1

                if tasks:
                    for task in asyncio.as_completed(tasks):
                        result = await task
                        if result is not None:
                            find_url.append(result)

    if find_url:
        latest_urls = get_urls(find_url)
        await pre_version(latest_urls)
    else:
        await pre_version(False)


if __name__ == "__main__":
    asyncio.run(main())
