#!/usr/bin/env python3
"""Writes assets/icons/open-icons.json, the icon set every build uses unless it asks for SF Symbols, from
Framework7 Icons (MIT): `npm pack framework7-icons`, untar it, then

    tools/open-icons.py path/to/package

Each glyph keeps Framework7's 56-unit box, scaled to the app's 230-unit icon box, with the path's own translate
baked in. `mirror` glyphs are flipped left to right; line icons get `b`, a stroke around the outline (in box units)
that brings them close to SF Symbols' semibold, which the app was drawn with (filled icons would lose their gaps).
"""
import json
import re
import sys
from pathlib import Path

BOX = 56.0
BOLD = 1.2
# the app's name -> (Framework7's, options)
GLYPHS = {
    'play': ('play_fill', ''), 'pause': ('pause_fill', ''), 'skip': ('forward_fill', ''),
    'rewind': ('backward_fill', ''), 'prevEp': ('backward_end_fill', ''), 'nextEp': ('forward_end_fill', ''),
    'thumbsUp': ('hand_thumbsup_fill', ''), 'thumbsDown': ('hand_thumbsdown_fill', ''),
    'thumbsUpOutline': ('hand_thumbsup', 'bold'), 'thumbsDownOutline': ('hand_thumbsdown', 'bold'),
    'star': ('star_fill', ''), 'starOutline': ('star', 'bold'), 'heart': ('heart_fill', ''),
    'plus': ('plus', 'bold'), 'check': ('checkmark', 'bold'), 'checkCircle': ('checkmark_circle', 'bold'),
    'bubble': ('captions_bubble_fill', ''), 'grid': ('square_grid_2x2_fill', ''), 'note': ('music_note', ''),
    'search': ('search', 'bold'), 'gear': ('gear_alt_fill', ''), 'info': ('info_circle', 'bold'),
    'power': ('power', 'bold'), 'sort': ('arrow_up_arrow_down', 'bold'), 'user': ('person_fill', ''),
    # an arrow leaving a box to the right, like SF's rectangle.portrait.and.arrow.right
    'logout': ('square_arrow_left', 'bold mirror'), 'back': ('chevron_left', 'bold'), 'edit': ('pencil', 'bold'),
    'chevronRight': ('chevron_right', 'bold'), 'chevronDown': ('chevron_down', 'bold'), 'trash': ('trash_fill', ''),
    'speaker': ('speaker_2_fill', ''), 'textSize': ('textformat_size', ''), 'photo': ('photo', 'bold'),
    'tv': ('tv', 'bold'), 'sparkles': ('sparkles', ''), 'xmark': ('xmark', 'bold'), 'clock': ('clock_fill', ''),
    'film': ('film_fill', ''), 'rows': ('rectangle_grid_1x2_fill', ''),
}
# coordinates each command takes, all of them pairs; Framework7's paths only use these, absolute
PAIRS = {'M': 1, 'L': 1, 'C': 3, 'Z': 0}


def number(value):
    text = f'{value:.3f}'.rstrip('0').rstrip('.')
    return '0' if text in ('', '-0') else text


def glyph(svg, mirror):
    path = re.search(r'<path[^>]*>', svg).group(0)
    d = re.search(r' d="([^"]*)"', path).group(1)
    dx, dy = map(float, re.search(r'translate\(([-\d.]+)[ ,]+([-\d.]+)\)', path).groups()) if 'translate' in path else (0, 0)
    out, command = [], None
    tokens = re.findall(r'[A-Za-z]|-?\d*\.?\d+(?:e-?\d+)?', d)
    i = 0
    while i < len(tokens):
        if tokens[i].isalpha():
            command = tokens[i]
            if command not in PAIRS:
                raise ValueError(f'unexpected command {command}')
            i += 1
            out.append(command)
            if command == 'Z':
                continue
        # a command's numbers, repeated while more follow (after M they mean L)
        for _ in range(PAIRS[command]):
            x, y = float(tokens[i]) + dx, float(tokens[i + 1]) + dy
            out.append(f'{number(BOX - x if mirror else x)} {number(y)}')
            i += 2
        if command == 'M':
            command = 'L'
    entry = {'w': BOX, 'h': BOX, 's': 230.0 / BOX, 'd': ' '.join(out).replace(' Z', 'Z').replace('Z ', 'Z')}
    if 'evenodd' in path:
        entry['e'] = True
    return entry


def main():
    package = Path(sys.argv[1])
    version = json.loads((package / 'package.json').read_text())['version']
    glyphs = {}
    for name, (source, options) in GLYPHS.items():
        entry = glyph((package / 'svg' / f'{source}.svg').read_text(), 'mirror' in options)
        if 'bold' in options:
            entry['b'] = BOLD
        glyphs[name] = entry
    out = Path(__file__).resolve().parent.parent / 'assets/icons/open-icons.json'
    data = {'source': f'Framework7 Icons {version} (MIT, Vladimir Kharlampidi), see assets/icons/README.md', 'glyphs': glyphs}
    out.write_text(json.dumps(data, indent=1) + '\n')
    print(f'{out}: {len(glyphs)} glyphs from framework7-icons {version}')


if __name__ == '__main__':
    main()
