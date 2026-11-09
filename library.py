"""File catalog and movie/show library, independent of Transmission membership."""
import base64
import hashlib
import json
import logging
import os
from pathlib import Path
import re
import threading
import time
import urllib.parse
import urllib.request
import uuid
import xml.etree.ElementTree as ET

from metadata import parse_name

VIDEO = {'.mp4','.m4v','.mkv','.mov','.avi','.webm','.ts','.m2ts','.mpg','.mpeg','.wmv'}
EPISODE = re.compile(r'(?i)(?<![a-z0-9])s(\d{1,2})[ ._-]*e(\d{1,3})(?!\d)|(?<!\d)(\d{1,2})x(\d{2})(?!\d)')
LOG = logging.getLogger('torrbaby')

class Library:
    def __init__(self, service):
        self.s = service
        self.wake = threading.Event()
        self.last_scan = None
        self.error = None
        self.scanning = False

    def allowed(self, path):
        path = Path(path)
        for key in ('source','movies','tv'):
            root = Path(self.s.config[key])
            try: relative = path.relative_to(root)
            except ValueError: continue
            if '..' in relative.parts: continue
            current = root
            for part in relative.parts:
                current = current / part
                if current.is_symlink(): raise ValueError('Symlinks are not served')
            return path
        raise ValueError('Media is outside library folders')

    def inventory(self):
        old = self.s.db.library_assets()
        by_path = {a['path']: a for a in old}
        by_torrent = {(a.get('torrent_hash'), a.get('file_index')): a for a in old if a.get('torrent_hash')}
        by_inode = {a.get('inode'): a for a in old if a.get('inode')}
        candidates = {}
        torrents = self.s.rpc.torrents()
        for t in torrents:
            for index, file in enumerate(t['files']):
                path = str(Path(t['download_dir']) / file['name'])
                if Path(path).suffix.lower() not in VIDEO: continue
                candidates[path] = {'torrent_hash': t['hash_string'], 'file_index': index, 'size': file['length'],
                                    'complete': file['length'] > 0 and file['bytes_completed'] == file['length'],
                                    'added_at': t.get('added_date', time.time()), 'torrent_name': t['name']}
        # Plex can enumerate indexed files even where macOS denies the daemon directory reads.
        try:
            sections = ET.fromstring(self.s.plex.request('/library/sections'))
            for section in sections.findall('Directory'):
                kind = section.get('type')
                if kind not in ('movie','show'): continue
                root = ET.fromstring(self.s.plex.request('/library/sections/' + section.get('key') + '/all?type=' + ('1' if kind == 'movie' else '4')))
                for video in root.findall('Video'):
                    for part in video.findall('.//Part'):
                        path = part.get('file')
                        if not path or Path(path).suffix.lower() not in VIDEO: continue
                        entry = candidates.setdefault(path, {'size': int(part.get('size',0)), 'complete': True, 'added_at': float(video.get('addedAt',time.time()))})
                        entry['plex_key'] = video.get('key')
                        entry['series_name'] = video.get('grandparentTitle')
                        if kind == 'show':
                            entry['season'] = int(video.get('parentIndex',0))
                            entry['episode'] = int(video.get('index',0))
        except Exception as error:
            LOG.warning('Plex inventory unavailable: %s',type(error).__name__)
        for key in ('movies','tv','source'):
            for directory, _, files in os.walk(self.s.config[key], followlinks=False):
                for name in files:
                    path = str(Path(directory)/name)
                    if Path(name).suffix.lower() in VIDEO and path not in candidates:
                        candidates[path] = {'complete': key != 'source', 'added_at': time.time()}
        seen = set()
        for path, info in candidates.items():
            try:
                p = self.allowed(path)
                stat = p.stat()
                inode = '%s:%s' % (stat.st_dev,stat.st_ino)
                if not p.is_file(): continue
            except (ValueError,OSError): continue
            previous = by_torrent.get((info.get('torrent_hash'),info.get('file_index'))) or by_path.get(path) or by_inode.get(inode)
            asset = dict(previous or {})
            asset.update(info)
            if not info.get('torrent_hash'):
                asset.pop('torrent_hash',None);asset.pop('file_index',None)
            asset.update(id=asset.get('id','asset-'+uuid.uuid4().hex),path=path,inode=inode,available=True,
                         size=info.get('size') or stat.st_size,seen_at=time.time())
            if previous: asset['added_at'] = previous['added_at']
            if 'season' not in asset:
                match = EPISODE.search(path)
                if match:
                    asset['season'] = int(match.group(1) or match.group(3))
                    asset['episode'] = int(match.group(2) or match.group(4))
            asset.setdefault('status','pending')
            self.s.db.save_library_asset(asset)
            seen.add(asset['id'])
        for asset in old:
            if asset['id'] not in seen and not Path(asset['path']).exists():
                asset['available'] = False
                self.s.db.save_library_asset(asset)
        self.last_scan = time.time()

    def resolve(self, asset):
        path = Path(asset['path'])
        name = asset.get('series_name') or path.name
        if asset.get('season') is not None and not EPISODE.search(name):
            name += ' S%02dE%02d' % (asset['season'],asset.get('episode',0))
        fake = {'name': name, 'download_dir': str(path.parent), 'files': [{'name':path.name,'length':asset['size']}], 'hash_string':asset['id']}
        result = self.s.metadata_collector.search(fake)
        selected = result.get('selected')
        if selected:
            kind = selected.get('kind') or ('tv' if asset.get('season') is not None else 'movie')
            entity_id = selected.get('id') or selected['provider'].lower().replace(' ','-') + ':' + kind + ':' + str(selected['provider_id'])
            selected.update(id=entity_id,kind=kind)
            self.s.db.save_library_entity(selected)
            asset['entity_id'] = entity_id
        asset.update(status=result['status'],candidates=result.get('candidates',[]),matched_at=time.time(),error=None)
        self.s.db.save_library_asset(asset)

    def run(self):
        while not self.s.stop.is_set():
            self.wake.clear()
            self.scanning = True
            try:
                self.inventory()
                self.error = None
                for asset in self.s.db.library_assets():
                    if self.s.stop.is_set(): return
                    if not asset.get('available'): continue
                    if asset.get('confirmed') or (asset.get('entity_id') and time.time()-asset.get('matched_at',0)<86400): continue
                    if time.time()-asset.get('matched_at',0)<900: continue
                    try: self.resolve(asset)
                    except Exception as error:
                        asset.update(status='error',error=type(error).__name__,matched_at=time.time())
                        self.s.db.save_library_asset(asset)
                    if self.s.stop.wait(.25): return
            except Exception as error:
                self.error = str(error)
                LOG.exception('Library scan failed')
            self.scanning = False
            self.wake.wait(60)

    def snapshot(self):
        entities = self.s.db.library_entities()
        history = {h['id']:h for h in self.s.db.history()}
        titles = {}
        for asset in self.s.db.library_assets():
            if not asset.get('available'): continue
            entity_id = asset.get('entity_id') or asset['id']
            entity = entities.get(entity_id) or {}
            fallback, year, kind = parse_name(Path(asset['path']).name)
            kind = entity.get('kind') or ('tv' if asset.get('season') is not None else kind)
            title = titles.setdefault(entity_id, {
                'id':entity_id,'kind':kind,'title':entity.get('title') or asset.get('series_name') or fallback,
                'original_title':entity.get('original_title'),'year':entity.get('year') or year,
                'release_date':entity.get('release_date') or str(entity.get('year') or year),
                'genres':entity.get('genres',[]),'summary':entity.get('summary') or '',
                'content_rating':entity.get('content_rating'),'duration_minutes':entity.get('duration_minutes'),
                'poster':entity.get('thumbnail'),'backdrop':entity.get('backdrop'),'logo':entity.get('logo'),
                'provider':entity.get('provider'),'source_url':entity.get('source_url'),
                'added_at':asset['added_at'],'assets':[],'status':asset['status']})
            title['added_at'] = max(title['added_at'],asset['added_at'])
            media_id = asset.get('torrent_hash',asset['id'])+':'+str(asset.get('file_index',0))
            progress = history.get(media_id,{})
            episode = next((e for e in entity.get('episodes',[]) if str(e.get('season'))==str(asset.get('season')) and str(e.get('episode'))==str(asset.get('episode'))),{}) if kind=='tv' else {}
            title['assets'].append({'id':asset['id'],'name':episode.get('title') or (entity.get('title') if kind=='movie' else Path(asset['path']).stem),
                'season':asset.get('season'),'episode':asset.get('episode'),'summary':episode.get('summary') or '',
                'airdate':episode.get('airdate'),'thumbnail':episode.get('thumbnail'),
                'duration_minutes':episode.get('duration_minutes') or entity.get('duration_minutes'),
                'complete':asset['complete'],'size':asset['size'],'position':progress.get('position',0),
                'duration':progress.get('duration',0),'watched':progress.get('watched',False),
                'last_played':progress.get('last_played',0),'torrent_hash':asset.get('torrent_hash')})
        return {'titles':sorted(titles.values(),key=lambda t:t['title'].casefold()),'last_scan':self.last_scan,'scanning':self.scanning,'error':self.error}

    def asset(self,id_):
        asset = next((a for a in self.s.db.library_assets() if a['id']==id_),None)
        if not asset: raise ValueError('Media file not found')
        return asset

    def media_file(self,id_):
        asset = self.asset(id_)
        if not asset.get('complete'): raise ValueError('Download is not complete')
        if asset.get('torrent_hash'):
            try: current = self.s.torrent(asset['torrent_hash'])
            except ValueError: current = None
            if current:
                file = current['files'][asset['file_index']]
                if file['bytes_completed'] != file['length']: raise ValueError('Download is not complete')
                asset['path'] = str(Path(current['download_dir'])/file['name'])
        path = self.allowed(asset['path'])
        if not path.is_file() or path.stat().st_size != asset['size']: raise ValueError('Media is unavailable or changed')
        return path

    def artwork(self, id_, variant):
        entity = self.s.db.library_entities().get(id_)
        if entity:
            url = entity.get({'poster':'thumbnail','backdrop':'backdrop','logo':'logo'}.get(variant,'thumbnail'))
            provider = entity.get('provider')
        else:
            asset = self.asset(id_)
            entity = self.s.db.library_entities().get(asset.get('entity_id'),{})
            episode = next((e for e in entity.get('episodes',[]) if str(e.get('season'))==str(asset.get('season')) and str(e.get('episode'))==str(asset.get('episode'))),{})
            url = episode.get('thumbnail') or entity.get('thumbnail')
            provider = entity.get('provider')
        if not url:return {'data':None}
        cache=self.s.path.parent/'artwork';cache.mkdir(mode=0o700,exist_ok=True)
        path=cache/(hashlib.sha256(url.encode()).hexdigest()+'.image')
        if path.exists():data=path.read_bytes()
        else:
            if provider=='Plex' and url.startswith('/library/metadata/'):response=self.s.plex.open(url)
            elif urllib.parse.urlsplit(url).hostname in ('image.tmdb.org','static.tvmaze.com'):
                response=urllib.request.urlopen(url,timeout=15)
            else:raise ValueError('Unsupported artwork source')
            with response:data=response.read(8*1024*1024+1)
            if len(data)>8*1024*1024:raise ValueError('Artwork too large')
            path.write_bytes(data)
        return {'data':base64.b64encode(data).decode()}
