import librosa
import pretty_midi
import sounddevice as sd
import soundfile as sf

def record_claps(filename="clap.wav", duration=8, samplerate=44100):
    """Record clapping audio from mic."""
    print(f"🎙️ Recording {duration} seconds... Clap steadily (like 1-2-3-4)!")
    audio = sd.rec(int(duration * samplerate), samplerate=samplerate, channels=1, dtype="float32")
    sd.wait()
    sf.write(filename, audio, samplerate)
    print(f"✅ Saved recording to {filename}")
    return filename

def clap_to_drum_groove(wav_file, out_midi="drum_groove.mid"):
    """Use clapping as tempo guide, generate a clean 4/4 drum groove."""
    # Load clap audio
    y, sr = librosa.load(wav_file)

    # Estimate tempo
    tempo, _ = librosa.beat.beat_track(y=y, sr=sr)
    if hasattr(tempo, "__len__"):  # sometimes returns array
        tempo = float(tempo[0])
    else:
        tempo = float(tempo)

    print(f"🎵 Detected tempo: {tempo:.1f} BPM")

    # MIDI setup
    pm = pretty_midi.PrettyMIDI(initial_tempo=tempo)
    drums = pretty_midi.Instrument(program=0, is_drum=True)

    beat_len = 60.0 / tempo   # seconds per beat
    num_bars = 8              # how many bars to generate
    total_beats = num_bars * 4

    for i in range(total_beats):
        t = i * beat_len
        beat = i % 4

        # Kick on 1 & 3
        if beat == 0 or beat == 2:
            drums.notes.append(pretty_midi.Note(110, 36, t, t+0.1))  # Kick drum

        # Snare on 2 & 4
        if beat == 1 or beat == 3:
            drums.notes.append(pretty_midi.Note(110, 38, t, t+0.1))  # Snare drum

        # Hi-hats every 8th note
        for off in [0.0, 0.5]:
            drums.notes.append(pretty_midi.Note(85, 42, t + off*beat_len, t + off*beat_len + 0.05))

    pm.instruments.append(drums)
    pm.write(out_midi)
    print(f"🥁 Drum groove saved as: {out_midi}")
    return out_midi

if __name__ == "__main__":
    # Step 1: Record your claps (used to detect tempo)
    wav = record_claps("clap.wav", duration=8)

    # Step 2: Generate structured groove MIDI
    midi_file = clap_to_drum_groove(wav, "drum_groove.mid")

    print("✅ Import 'drum_groove.mid' into BandLab, assign a drum kit, and enjoy a real groove!")