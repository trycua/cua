import sounddevice as sd
import soundfile as sf
import numpy as np
def record_hum(output_file="hum.wav", samplerate=44100, channels=1, silence_thresh=0.01, min_silence_len=2):
    print("🎤 Ready. Start humming...")

    recording = []
    silence_counter = 0
    chunk_size = int(samplerate * 0.1)  # 100ms chunks

    with sd.InputStream(samplerate=samplerate, channels=channels) as stream:
        while True:
            chunk, _ = stream.read(chunk_size)
            chunk = chunk.copy()
            recording.append(chunk)

            volume = np.abs(chunk).mean()
            if volume < silence_thresh:
                silence_counter += 1
            else:
                silence_counter = 0

            if silence_counter > min_silence_len * 10:  # e.g. 2s silence
                break

    recording = np.concatenate(recording, axis=0)
    sf.write(output_file, recording, samplerate)
    print(f"✅ Saved recording to {output_file}")

if __name__ == "__main__":
    record_hum()