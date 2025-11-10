import json
import re

data_json = "${{ github.event.inputs.datajson }}"
data = json.loads(data_json)

with open('versions.json', 'r') as f:
    file_data = json.load(f)

key = list(data.keys())[0]
file_data[key] = data[key]

file_data = dict(
    sorted(
        file_data.items(),
        key=lambda x: [
            int(part) if part.isdigit() else part for part in re.split(r'(\d+)', x[0])
        ],
        reverse=True
    )
)

with open('versions.json', 'w') as f:
    json.dump(file_data, f, indent=2)