from basic_pitch.inference import predict_and_save, ICASSP_2022_MODEL_PATH

def convert_to_midi(input_wav: str, output_dir: str = ".") -> str:
    """
    Convert a WAV file into a MIDI using Spotify's Basic Pitch.
    
    Args:
        input_wav (str): Path to input WAV file
        output_dir (str): Directory where outputs will be saved
    
    Returns:
        str: Path to generated MIDI file
    """
    predict_and_save(
        [input_wav],                # list of audio files
        output_dir,                 # where to save results
        sonify_midi=False,          # don't create audio preview
        save_midi=True,             # save the MIDI file
        save_model_outputs=False,   # skip raw model outputs
        model_or_model_path=ICASSP_2022_MODEL_PATH,
        save_notes=True             # also save note array (.csv)
    )

    # Basic Pitch appends `_basic_pitch.mid` to the filename
    midi_path = f"{output_dir}/{input_wav.split('/')[-1].replace('.wav', '_basic_pitch.mid')}"
    print(f"✅ Converted {input_wav} → {midi_path}")
    return midi_path


# Example usage
if __name__ == "__main__":
    midi_file = convert_to_midi("hum.wav", ".")
    print("Generated MIDI:", midi_file)