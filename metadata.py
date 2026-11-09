"""Personal-library metadata collection; provider data stays separate from filenames."""
import html
import base64
import hashlib
import json
from pathlib import Path
import re
import time
import urllib.parse
import urllib.request
import urllib.error
import xml.etree.ElementTree as ET

SEASON = re.compile(r'(?i)(?<![a-z0-9])(?:s\d{1,2}(?:[ ._-]*e\d{1,3})?|season[ ._-]*\d+|\d{1,2}x\d{2})')
YEAR = re.compile(r'(?<!\d)((?:19|20)\d{2})(?!\d)')

def plain(value):
    return html.unescape(re.sub('<[^>]+>', '', value or '')).strip()

def normalized(value):
    return re.sub(r'[^\w]', '', value.casefold())

def parse_name(name):
    name = re.sub(r'(?i)^www\.[^ ]+\s*[- ]*', '', name)
    marker, year = SEASON.search(name), YEAR.search(name)
    cut = min([x.start() for x in (marker, year) if x] or [len(name)])
    title = re.sub(r'[._]', ' ', name[:cut]).strip(' -()[]')
    title = re.split(r'(?i)\b(?:complete|collection|trilogy|1080p|2160p|720p|bluray|webrip|web-dl)\b', title)[0].strip(' -()[]')
    return title, year.group(1) if year else '', 'tv' if marker else 'movie'

def fetch(url):
    with urllib.request.urlopen(urllib.request.Request(url, headers={'User-Agent': 'Torrbaby/0.2 (personal library)'}), timeout=15) as r:
        return json.load(r)

class MetadataCollector:
    def __init__(self, service):
        self.s = service
        self.plex_index = {}
        self.plex_updated = 0
        self.token_path = self.s.path.parent / "tmdb.token"
        for record in self.s.db.metadata():
            if record.get('selected') and ('match_confidence' not in record['selected'] or record['selected'].get('match_confidence') == 'plex_file_match') and not record.get('confirmed'):
                self.s.db.refresh_metadata(record['hash'])

    def plex(self, torrent):
        if time.time() - self.plex_updated > 300:
            index = {}
            root = ET.fromstring(self.s.plex.request('/library/sections'))
            for section in root.findall('Directory'):
                kind = section.get('type')
                if kind not in ('movie', 'show'): continue
                path = '/library/sections/' + section.get('key') + '/all?type=' + ('1' if kind == 'movie' else '4')
                for video in ET.fromstring(self.s.plex.request(path)).findall('Video'):
                    for part in video.findall('.//Part'):
                        if part.get('file'): index[part.get('file')] = video
            self.plex_index, self.plex_updated = index, time.time()
        files = []
        for i, file in enumerate(torrent['files']):
            video = self.plex_index.get(str(Path(torrent['download_dir']) / file['name']))
            if video is None: continue
            files.append({'file_index': i, 'title': video.get('title'), 'series': video.get('grandparentTitle'),
                          'season': video.get('parentIndex'), 'episode': video.get('index'),
                          'duration_minutes': round(float(video.get('duration', 0))/60000, 1),
                          'content_rating': video.get('contentRating'), 'plex_key': video.get('key')})
        if not files: return None
        video = self.plex_index[str(Path(torrent['download_dir']) / torrent['files'][files[0]['file_index']]['name'])]
        # Show-level genres/artwork/title belong to the series, not its first episode.
        if video.get('grandparentKey'):
            detail = ET.fromstring(self.s.plex.request(video.get('grandparentKey')))
            source = detail.find('Directory')
            if source is None: source = video
        else:
            detail = ET.fromstring(self.s.plex.request(video.get('key')))
            source = detail.find('Video')
            if source is None: source = video
        return {'provider': 'Plex', 'provider_id': source.get('ratingKey'), 'title': source.get('title'),
                'year': source.get('year', ''), 'genres': [g.get('tag') for g in source.findall('Genre')],
                'content_rating': source.get('contentRating'), 'duration_minutes': round(float(source.get('duration', 0))/60000, 1),
                'summary': source.get('summary', ''), 'thumbnail': source.get('thumb'), 'logo': None,
                'source_url': None, 'episodes': files, 'files': files, 'match_confidence': 'plex_file_match'}

    def tmdb(self, path, token_override=None, **query):
        token = token_override or self.token_path.read_text().strip()
        headers = {'User-Agent': 'Torrbaby/0.2', 'Accept': 'application/json'}
        if re.fullmatch(r'[a-fA-F0-9]{32}', token): query['api_key'] = token
        else: headers['Authorization'] = 'Bearer ' + token
        url = 'https://api.themoviedb.org/3/' + path + '?' + urllib.parse.urlencode(query)
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=15) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            code = error.code
            error.close()
            raise RuntimeError('TMDB HTTP ' + str(code)) from None

    def configure_tmdb(self, token):
        if not isinstance(token, str) or not 20 <= len(token.strip()) <= 4096 or any(c.isspace() for c in token.strip()):
            raise ValueError('Enter a TMDB API key or Read Access Token')
        self.tmdb('configuration', token_override=token.strip())
        tmp = self.token_path.with_suffix('.tmp')
        with tmp.open('w') as file:
            import os
            os.fchmod(file.fileno(), 0o600)
            file.write(token.strip())
        tmp.replace(self.token_path)
        # Revisit automatic keyless matches with the richer source. Preserve explicit choices.
        for record in self.s.db.metadata():
            if not record.get('confirmed'): self.s.db.refresh_metadata(record['hash'])
        return {'configured': True}

    def search(self, torrent):
        query, year, kind = parse_name(torrent['name'])
        if any(SEASON.search(f['name']) for f in torrent.get('files', [])): kind = 'tv'
        if not query: return {'status': 'no_match', 'query': query, 'candidates': [], 'selected': None}
        try:
            local = self.plex(torrent)
        except Exception:
            local = None
        if local:
            local['match_confidence'] = 'exact_plex_title_year' if normalized(local['title']) == normalized(query) and (not year or local['year'] == year) else None
        if local and not self.token_path.exists():
            return {'status': 'matched', 'query': query, 'candidates': [local], 'selected': local}
        if local:
            query, year = local["title"], local["year"]
        candidates = []
        if self.token_path.exists():
            data = self.tmdb('search/' + kind, query=query, language='en-US', include_adult='false')
            for hit in data.get('results', [])[:8]:
                candidates.append({'provider': 'TMDB', 'provider_id': str(hit['id']), 'kind': kind,
                    'title': hit.get('title') or hit.get('name'), 'original_title': hit.get('original_title') or hit.get('original_name'),
                    'year': (hit.get('release_date') or hit.get('first_air_date') or '')[:4], 'genres': [],
                    'content_rating': None, 'duration_minutes': None, 'summary': hit.get('overview', ''),
                    'thumbnail': 'https://image.tmdb.org/t/p/w500' + hit['poster_path'] if hit.get('poster_path') else None,
                    'logo': None, 'source_url': 'https://www.themoviedb.org/' + kind + '/' + str(hit['id']), 'episodes': []})
        elif kind == 'tv':
            data = fetch('https://api.tvmaze.com/search/shows?' + urllib.parse.urlencode({'q': query}))
            for hit in data[:6]:
                show = hit['show']
                candidates.append({'provider': 'TVmaze', 'provider_id': str(show['id']), 'title': show['name'],
                    'year': (show.get('premiered') or '')[:4], 'genres': show.get('genres', []),
                    'duration_minutes': show.get('averageRuntime') or show.get('runtime'), 'content_rating': None,
                    'summary': plain(show.get('summary')), 'thumbnail': (show.get('image') or {}).get('medium'),
                    'logo': None, 'source_url': show.get('url'), 'episodes': []})
        else:
            data = fetch('https://itunes.apple.com/search?' + urllib.parse.urlencode({'term': query, 'entity': 'movie', 'country': 'US', 'limit': 8}))
            for movie in data.get('results', []):
                candidates.append({'provider': 'Apple iTunes Search', 'provider_id': str(movie['trackId']), 'title': movie['trackName'],
                    'year': (movie.get('releaseDate') or '')[:4], 'genres': [movie['primaryGenreName']] if movie.get('primaryGenreName') else [],
                    'duration_minutes': round(movie.get('trackTimeMillis', 0)/60000, 1), 'content_rating': movie.get('contentAdvisoryRating'),
                    'summary': movie.get('longDescription') or movie.get('shortDescription') or '',
                    'thumbnail': None, 'logo': None, 'source_url': movie.get('trackViewUrl'), 'episodes': []})
        exact = [c for c in candidates if normalized(query) in (normalized(c['title']), normalized(c.get('original_title') or '')) and (not year or c['year'] == year)]
        selected = self.enrich(exact[0]) if len(exact) == 1 else None
        if selected: selected['match_confidence'] = 'exact_title_year' if year else 'exact_unique_title'
        return {'status': 'matched' if selected else ('review' if candidates else 'no_match'), 'query': query,
                'candidates': candidates, 'selected': selected}

    def enrich(self, candidate):
        candidate = dict(candidate)
        if candidate['provider'] == 'TMDB':
            kind = candidate['kind']
            detail = self.tmdb(kind + '/' + str(int(candidate['provider_id'])), language='en-US', append_to_response='images,release_dates,content_ratings', include_image_language='en,zh,null')
            candidate['genres'] = [g['name'] for g in detail.get('genres', [])]
            candidate['duration_minutes'] = detail.get('runtime') or next(iter(detail.get('episode_run_time', [])), None)
            logos = detail.get('images', {}).get('logos', [])
            candidate['logo'] = 'https://image.tmdb.org/t/p/w500' + logos[0]['file_path'] if logos else None
            ratings = detail.get('release_dates' if kind == 'movie' else 'content_ratings', {}).get('results', [])
            for rating in ratings:
                if rating.get('iso_3166_1') == 'US':
                    candidate['content_rating'] = rating.get('rating') if kind == 'tv' else next((r['certification'] for r in rating.get('release_dates', []) if r.get('certification')), None)
            if kind == 'tv':
                candidate['episodes'] = []
                for season in detail.get('seasons', []):
                    if self.s.stop.is_set(): break
                    data = self.tmdb('tv/' + str(int(candidate['provider_id'])) + '/season/' + str(season['season_number']), language='en-US')
                    candidate['episodes'].extend({'title': e['name'], 'season': e['season_number'], 'episode': e['episode_number'], 'duration_minutes': e.get('runtime'), 'airdate': e.get('air_date')} for e in data.get('episodes', []))
        elif candidate['provider'] == 'TVmaze':
            episodes = fetch('https://api.tvmaze.com/shows/' + str(int(candidate['provider_id'])) + '/episodes?specials=1')
            candidate['episodes'] = [{'title': e['name'], 'season': e.get('season'), 'episode': e.get('number'),
                                     'duration_minutes': e.get('runtime'), 'airdate': e.get('airdate'),
                                     'thumbnail': (e.get('image') or {}).get('medium'), 'source_url': e.get('url')}
                                    for e in episodes]
        return candidate

    def artwork(self, hash_):
        record = next((r for r in self.s.db.metadata() if r['hash'] == hash_), None)
        selected = (record or {}).get('selected') or {}
        url = selected.get('thumbnail')
        if not url: return {'data': None}
        cache = self.s.path.parent / 'artwork'
        cache.mkdir(mode=0o700, exist_ok=True)
        path = cache / (hashlib.sha256(url.encode()).hexdigest() + '.image')
        if path.exists(): data = path.read_bytes()
        else:
            if selected.get('provider') == 'Plex' and url.startswith('/library/metadata/'):
                response = self.s.plex.open(url)
            elif urllib.parse.urlsplit(url).hostname in ('static.tvmaze.com', 'image.tmdb.org'):
                response = urllib.request.urlopen(url, timeout=15)
            else: raise ValueError('Unsupported artwork source')
            with response:
                data = response.read(5 * 1024 * 1024 + 1)
            if len(data) > 5 * 1024 * 1024: raise ValueError('Artwork exceeds size limit')
            path.write_bytes(data)
        return {'data': base64.b64encode(data).decode()}

    def run(self):
        while not self.s.stop.is_set():
            try:
                torrents = sorted(self.s.rpc.torrents(), key=lambda t: (t.get('percent_done', 0) >= 1, -t.get('added_date', 0)))
                existing = {m['hash']: m for m in self.s.db.metadata()}
                for torrent in torrents:
                    if self.s.stop.is_set(): return
                    old = existing.get(torrent['hash_string'])
                    # Keep confirmed choices. Retry missing results after a day; explicit refresh is available.
                    if old and (old.get('confirmed') or time.time() - old['updated'] < (86400 if old['status'] != 'error' else 900)):
                        continue
                    try:
                        result = self.search(torrent)
                    except Exception as e:
                        result = {'status': 'error', 'error': str(e), 'candidates': [], 'selected': None}
                    result.update(hash=torrent['hash_string'], torrent_name=torrent['name'], updated=time.time(), confirmed=False)
                    self.s.db.save_metadata(result)
                    if self.s.stop.wait(5): return
            except Exception:
                import logging
                logging.getLogger('torrbaby').exception('Metadata collection failed')
            self.s.stop.wait(30)
