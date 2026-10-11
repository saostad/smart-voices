import Testing
@testable import SmartVoicesCore

@Suite
struct AudioStitcherTests {
    @Test func stitchesSamplesWithSilence() throws {
        let stitched = try AudioStitcher.stitch(
            [
                SynthesizedAudio(samples: [1, 2, 3], sampleRate: 24000),
                SynthesizedAudio(samples: [4, 5], sampleRate: 24000),
            ],
            pauses: [0.5, 0])
        #expect(stitched.sampleRate == 24000)
        #expect(stitched.samples.count == 3 + 12000 + 2)
        #expect(Array(stitched.samples.prefix(3)) == [1, 2, 3])
        #expect(Array(stitched.samples.suffix(2)) == [4, 5])
        #expect(stitched.samples[3] == 0)
        #expect(stitched.samples[3 + 12000 - 1] == 0)
    }

    @Test func zeroPausesConcatenate() throws {
        let stitched = try AudioStitcher.stitch(
            [
                SynthesizedAudio(samples: [1], sampleRate: 22050),
                SynthesizedAudio(samples: [2, 3], sampleRate: 22050),
            ],
            pauses: [0, 0])
        #expect(stitched.samples == [1, 2, 3])
        #expect(stitched.sampleRate == 22050)
    }

    @Test func mismatchedSampleRatesThrow() {
        #expect(throws: AudioStitcherError.self) {
            try AudioStitcher.stitch(
                [
                    SynthesizedAudio(samples: [1], sampleRate: 24000),
                    SynthesizedAudio(samples: [2], sampleRate: 22050),
                ],
                pauses: [0, 0])
        }
    }

    @Test func mismatchedCountsThrow() {
        #expect(throws: AudioStitcherError.self) {
            try AudioStitcher.stitch(
                [SynthesizedAudio(samples: [1], sampleRate: 24000)], pauses: [0, 0])
        }
    }

    @Test func emptyStitchesToEmpty() throws {
        let stitched = try AudioStitcher.stitch([], pauses: [])
        #expect(stitched.samples.isEmpty)
        #expect(stitched.sampleRate == 24000)
    }
}
