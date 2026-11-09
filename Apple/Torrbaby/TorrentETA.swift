import Foundation

/// Time-weighted rate smoothing: a two-minute time constant gives roughly a
/// six-minute effective history while still responding to sustained changes.
/// Kept in Store so row reuse/navigation never resets an estimate.
struct TorrentETASmoother {
    private struct Sample {
        var started: Double
        var updated: Double
        var lastActivity: Double
        var rate: Double
    }
    private var samples: [String: Sample] = [:]
    private var source = ""

    mutating func update(_ torrents: [Torrent], server: String, now: Double) -> [Torrent] {
        if source != server { samples.removeAll(); source = server }
        let ids = Set(torrents.map(\.id))
        samples = samples.filter { ids.contains($0.key) }
        return torrents.map { original in
            var torrent = original
            guard !torrent.completed, torrent.status == 4 else {
                samples.removeValue(forKey: torrent.id)
                torrent.smoothedEta = nil
                return torrent
            }
            // Prefer the shared server estimate once the Rust server supplies it.
            if original.smoothedEta != nil { return original }
            let speed = Double(max(0, torrent.rateDownload))
            guard var sample = samples[torrent.id], now >= sample.updated, now - sample.updated < 60 else {
                samples[torrent.id] = Sample(started: now, updated: now, lastActivity: now, rate: speed)
                torrent.smoothedEta = -1
                return torrent
            }
            let elapsed = now - sample.updated
            let weight = 1 - exp(-elapsed / 120)
            sample.rate += weight * (speed - sample.rate)
            sample.updated = now
            if speed > 0 { sample.lastActivity = now }
            samples[torrent.id] = sample
            let seconds = Double(max(0, torrent.size - torrent.downloaded)) / max(1, sample.rate)
            if now - sample.started >= 15, now - sample.lastActivity < 30, !torrent.isStalled,
               sample.rate > 0, seconds.isFinite, seconds < Double(Int.max) {
                torrent.smoothedEta = Int(seconds.rounded())
            } else { torrent.smoothedEta = -1 }
            return torrent
        }
    }
}
