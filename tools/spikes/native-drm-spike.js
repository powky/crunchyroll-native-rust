// Native DRM playback spike for the LG C1 (node 8 on the TV). Reads /tmp/crspike/cap.json
// ({authorization, contentId}) captured from the web app, asks Crunchyroll for a stream, runs a local
// PlayReady/Widevine license proxy that adds Crunchyroll's headers, and plays the DASH manifest through
// uMediaServer (com.webos.media) with a com.webos.service.drm client. Never prints tokens.
'use strict'
const https = require('https')
const http = require('http')
const cp = require('child_process')
const fs = require('fs')
const url = require('url')

const DRM = process.argv[2] || 'playready'  // or widevine
const SECS = Number(process.argv[3] || 30)
const cap = JSON.parse(fs.readFileSync('/tmp/crspike/cap.json', 'utf8'))
const APP = 'com.crunchyroll.stream.app'
const log = (...a) => console.log(new Date().toISOString().slice(11, 23), ...a)

// the same client the TV web app says it is (another one gets "Outdated Token")
const UA = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0'

function request(method, target, headers, body) {
    headers = Object.assign({ 'User-Agent': UA }, headers)
    return new Promise((resolve, reject) => {
        const u = url.parse(target)
        const lib = u.protocol === 'https:' ? https : http
        const r = lib.request({ method, hostname: u.hostname, port: u.port, path: u.path, headers }, res => {
            const chunks = []
            res.on('data', c => chunks.push(c))
            res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, body: Buffer.concat(chunks) }))
        })
        r.on('error', reject)
        if (body) r.write(body)
        r.end()
    })
}

function luna(uri, payload) {
    return new Promise(resolve => {
        cp.execFile('luna-send', ['-n', '1', uri, JSON.stringify(payload)], { timeout: 15000 }, (err, out) => {
            try { resolve(JSON.parse(out)) } catch (e) { resolve({ raw: String(out), err: String(err) }) }
        })
    })
}

function subscribe(uri, payload, onLine) {
    const p = cp.spawn('luna-send', ['-i', uri, JSON.stringify(payload)], { stdio: ['ignore', 'pipe', 'pipe'] })
    let buf = ''
    p.stdout.on('data', d => {
        buf += d
        let i
        while ((i = buf.indexOf('\n')) >= 0) {
            const line = buf.slice(0, i).trim()
            buf = buf.slice(i + 1)
            if (line) onLine(line)
        }
    })
    return p
}

async function main() {
    const auth = cap.authorization
    const guid = cap.contentId
    log('content', guid, 'drm', DRM)

    // 1. the stream: the one the web app opened (captured), or one of our own like the TV app asks for it
    let stream = null
    if (cap.mpd && cap.videoToken && process.argv[4] !== 'own') {
        stream = { url: cap.mpd, token: cap.videoToken }
        log('reusing the web app stream; manifest host:', url.parse(stream.url).host)
    } else {
        const play = await request('GET', `https://cr-play-service.prd.crunchyrollsvc.com/v1/${guid}/tv/lg/play`,
            { Authorization: auth })
        if (play.status !== 200) {
            log('play failed', play.status, play.body.toString().slice(0, 300))
            return
        }
        stream = JSON.parse(play.body.toString())
        log('play ok; keys:', Object.keys(stream).join(','), '| manifest host:', url.parse(stream.url).host)
    }
    // the manifest needs Crunchyroll's Authorization too: the player gets it from the local server
    const mpd = await request('GET', stream.url, { Authorization: auth })
    let mpdText = mpd.body.toString()
    const uniq = (m) => (m || []).filter((v, i, arr) => arr.indexOf(v) === i)
    const red = (u) => u.replace(/[A-Za-z0-9_~.=%-]{20,}/g, '<x>').slice(0, 110)
    log('mpd', mpd.status, mpdText.length, 'bytes; ContentProtection:', uniq(mpdText.match(/schemeIdUri="[^"]+"/g)).join(' '))
    log('codecs:', uniq(mpdText.match(/codecs="[^"]+"/g)).join(' '))
    log('BaseURL:', uniq(mpdText.match(/<BaseURL>[^<]*<\/BaseURL>/g)).map(red).join(' | ') || 'none')
    log('templates:', uniq(mpdText.match(/(media|initialization)="[^"]+"/g)).slice(0, 4).map(red).join(' | '))
    log('types:', uniq(mpdText.match(/(type|profiles)="[^"]+"/g)).join(' '))
    // segments relative to the manifest resolve against its real address, not the local server's
    const base = stream.url.replace(/[^/]*(\?.*)?$/, '')
    if (!/<BaseURL>/.test(mpdText)) {
        mpdText = mpdText.replace(/<Period([^>]*)>/, `<Period$1><BaseURL>${base.replace(/&/g, '&amp;')}</BaseURL>`)
    }
    fs.writeFileSync('/tmp/crspike/manifest.mpd', mpdText)
    const fullMpd = mpdText
    // diagnostics: segments through the local server too, so every request the player makes is logged
    const segUrls = []
    if (process.argv.indexOf('proxyseg') >= 0) {
        mpdText = mpdText.replace(/<BaseURL>([^<]*)<\/BaseURL>/g, (m, u) => {
            segUrls.push(u.replace(/&amp;/g, '&'))
            return `<BaseURL>http://127.0.0.1:18080/seg/${segUrls.length - 1}/media.mp4</BaseURL>`
        })
        log('segments proxied:', segUrls.length)
    }
    if (process.argv.indexOf('strip') >= 0) {  // test: the player reads the encryption from the init segments instead
        mpdText = mpdText.replace(/<ContentProtection[\s\S]*?<\/ContentProtection>/g, '').replace(/<ContentProtection[^>]*\/>/g, '')
        log('ContentProtection stripped from the served manifest')
    }

    // 2. license proxy: the TV's DRM client posts its challenge here; Crunchyroll's headers are added
    const licenseUrl = `https://cr-license-proxy.prd.crunchyrollsvc.com/v1/license/${DRM === 'playready' ? 'playReady' : 'widevine'}`
    const server = http.createServer((req, res) => {
        const segm = req.url.match(/^\/seg\/(\d+)\//)
        if (segm) {
            const target = url.parse(segUrls[Number(segm[1])])
            log('segment', segm[1], req.method, 'range', req.headers.range || '-')
            const up = https.request({ method: req.method, hostname: target.hostname, path: target.path,
                headers: Object.assign({ 'User-Agent': UA }, req.headers.range ? { Range: req.headers.range } : {}) }, ures => {
                res.writeHead(ures.statusCode, ures.headers)
                ures.pipe(res)
            })
            up.on('error', e => { log('segment error', String(e)); res.writeHead(502); res.end() })
            up.end()
            return
        }
        if (req.method === 'GET' && req.url.indexOf('/manifest.mpd') === 0) {
            log('manifest requested by the player')
            res.writeHead(200, { 'Content-Type': 'application/dash+xml' })
            res.end(mpdText)
            return
        }
        const chunks = []
        req.on('data', c => chunks.push(c))
        req.on('end', async () => {
            const body = Buffer.concat(chunks)
            log('license request', req.method, req.url, body.length, 'bytes; content-type', req.headers['content-type'])
            const headers = {
                Authorization: auth,
                'X-Cr-Content-Id': guid,
                'X-Cr-Video-Token': stream.token,
                'Content-Type': DRM === 'playready' ? 'text/xml; charset=utf-8' : 'application/octet-stream',
            }
            if (DRM === 'playready') {
                headers.SOAPAction = '"http://schemas.microsoft.com/DRM/2007/03/protocols/AcquireLicense"'
            }
            try {
                const lic = await request('POST', licenseUrl, headers, body)
                let out = lic.body
                if (DRM !== 'playready' && lic.status === 200) {  // Crunchyroll wraps Widevine licenses in JSON
                    try { out = Buffer.from(JSON.parse(lic.body.toString()).license, 'base64') } catch (e) { /* raw */ }
                }
                log('license response', lic.status, out.length, 'bytes', lic.status !== 200 ? lic.body.toString().slice(0, 200) : '')
                res.writeHead(lic.status, { 'Content-Type': lic.headers['content-type'] || 'application/octet-stream' })
                res.end(out)
            } catch (e) {
                log('license proxy error', String(e))
                res.writeHead(502)
                res.end()
            }
        })
    })
    await new Promise(r => server.listen(18080, '127.0.0.1', r))
    log('license proxy on 127.0.0.1:18080')

    // 3. the TV's DRM client
    const drm = await luna('luna://com.webos.service.drm/load', { drmType: DRM, appId: APP })
    log('drm load', JSON.stringify(drm))
    const clientId = drm.clientId
    const rights = subscribe('luna://com.webos.service.drm/getRightsError', { clientId, subscribe: true },
        line => log('rightsError:', line.slice(0, 300)))
    let msg
    if (DRM === 'playready') {
        // the content's own PlayReady header (in the manifest's PlayReady Object), pointed at the local proxy:
        // the DRM service acquires the license with it before the player starts
        const pro = (fullMpd.match(/<mspr:pro[^>]*>([^<]+)<\/mspr:pro>/) || [])[1]
        let wrm = null
        if (pro) {
            const buf = Buffer.from(pro.trim(), 'base64')
            const count = buf.readUInt16LE(4)
            let off = 6
            for (let i = 0; i < count; i++) {
                const type = buf.readUInt16LE(off)
                const len = buf.readUInt16LE(off + 2)
                if (type === 1) {
                    wrm = buf.slice(off + 4, off + 4 + len).toString('utf16le')
                }
                off += 4 + len
            }
        }
        log('PlayReady header found:', !!wrm, wrm ? (wrm.match(/version="[^"]+"/) || [''])[0] + ' LA_URL host ' + ((wrm.match(/<LA_URL>https?:\/\/([^/<]+)/) || [])[1] || 'none') : '')
        const header = wrm
            ? (/<LA_URL>/.test(wrm)
                ? wrm.replace(/<LA_URL>[^<]*<\/LA_URL>/, '<LA_URL>http://127.0.0.1:18080/pr</LA_URL>')
                : wrm.replace(/<DATA>/, '<DATA><LA_URL>http://127.0.0.1:18080/pr</LA_URL>'))
            : ''
        msg = await luna('luna://com.webos.service.drm/sendDrmMessage', {
            clientId,
            msgType: 'application/vnd.ms-playready.initiator+xml',
            drmSystemId: 'urn:dvb:casystemid:19219',
            msg: '<?xml version="1.0" encoding="utf-8"?>'
                + '<PlayReadyInitiator xmlns="http://schemas.microsoft.com/DRM/2007/03/protocols/">'
                + `<LicenseAcquisition><Header>${header}</Header></LicenseAcquisition>`
                + '</PlayReadyInitiator>',
        })
    } else {
        msg = await luna('luna://com.webos.service.drm/sendDrmMessage', {
            clientId,
            msgType: 'application/widevine+xml',
            drmSystemId: 'urn:dvb:casystemid:19156',
            msg: '<?xml version="1.0" encoding="utf-8"?>'
                + '<WidevineCredentialsInfo xmlns="http://www.smarttv-alliance.org/DRM/widevine/2012/protocols/">'
                + `<ContentURL>${stream.url.replace(/&/g, '&amp;')}</ContentURL>`
                + '<DeviceID></DeviceID><StreamID></StreamID><ClientIP></ClientIP>'
                + '<DRMServerURL>http://127.0.0.1:18080/wv</DRMServerURL>'
                + '<DRMAckServerURL></DRMAckServerURL><DRMHeartBeatURL></DRMHeartBeatURL>'
                + '<DRMHeartBeatPeriod>0</DRMHeartBeatPeriod><UserData></UserData><Portal></Portal>'
                + '<StoreFront></StoreFront><BandwidthCheckURL></BandwidthCheckURL>'
                + '<BandwidthCheckInterval></BandwidthCheckInterval></WidevineCredentialsInfo>',
        })
    }
    log('sendDrmMessage', JSON.stringify(msg))

    // 4. the hardware player
    const load = await luna('luna://com.webos.media/load', {
        uri: 'http://127.0.0.1:18080/manifest.mpd',
        type: 'media',
        payload: { option: { appId: APP, drm: { type: DRM, clientId } } },
    })
    log('media load', JSON.stringify(load))
    const mediaId = load.mediaId
    let last = ''
    const sub = subscribe('luna://com.webos.media/subscribe', { mediaId }, line => {
        const key = (line.match(/"([a-zA-Z]+)":/) || [])[1] || line.slice(0, 20)
        if (key === 'bufferRange') return
        if (key === 'currentTime') {
            const t = (line.match(/"currentTime":(\d+)/) || [])[1]
            if (t !== last && Number(t) % 3000 < 1000) log('currentTime', t)
            last = t
            return
        }
        log('event:', line.slice(0, 260))
    })
    log('media play', JSON.stringify(await luna('luna://com.webos.media/play', { mediaId })))
    await new Promise(r => setTimeout(r, SECS * 1000))

    // 5. clean up: player, DRM client, proxy, and the stream slot at Crunchyroll
    log('media unload', JSON.stringify(await luna('luna://com.webos.media/unload', { mediaId })))
    sub.kill()
    rights.kill()
    log('drm unload', JSON.stringify(await luna('luna://com.webos.service.drm/unload', { clientId })))
    server.close()
    if (!cap.videoToken) {  // a captured stream belongs to the web app, which deletes it itself
        const del = await request('DELETE', `https://cr-play-service.prd.crunchyrollsvc.com/v1/token/${guid}/${stream.token}`,
            { Authorization: auth })
        log('stream token deleted', del.status)
    }
}

main().catch(e => log('fatal', e && e.stack || e)).then(() => process.exit(0))
