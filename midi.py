import os
import time
import shutil
from basic_pitch.inference import predict_and_save, ICASSP_2022_MODEL_PATH

def convert_to_midi(input_wav: str, output_dir: str = ".") -> str:
    """
    Convert a WAV file into a MIDI using Spotify's Basic Pitch.
    Creates timestamped MIDI files to avoid conflicts.
    
    Args:
        input_wav (str): Path to input WAV file
        output_dir (str): Directory where outputs will be saved
    
    Returns:
        str: Path to generated timestamped MIDI file
    """
    os.makedirs(output_dir, exist_ok=True)
    
    # Create timestamp for unique filenames
    timestamp = int(time.time())
    
    # Create temporary directory to avoid file conflicts
    temp_dir = os.path.join(output_dir, f"temp_midi_{timestamp}")
    os.makedirs(temp_dir, exist_ok=True)

    try:
        print(f"🎵 Converting {input_wav} to MIDI with timestamp {timestamp}")
        
        predict_and_save(
            [input_wav],                # list of audio files
            temp_dir,                   # temporary directory to avoid conflicts
            sonify_midi=False,          # don't create audio preview
            save_midi=True,             # save the MIDI file
            save_model_outputs=False,   # skip raw model outputs
            model_or_model_path=ICASSP_2022_MODEL_PATH,
            save_notes=True             # also save note array (.csv)
        )

        # Basic Pitch creates files with `_basic_pitch.mid` suffix
        base_name = os.path.splitext(os.path.basename(input_wav))[0]
        temp_midi_path = os.path.join(temp_dir, f"{base_name}_basic_pitch.mid")
        
        if not os.path.exists(temp_midi_path):
            raise FileNotFoundError(f"MIDI not found: {temp_midi_path}")
        
        # Create final timestamped filename
        final_midi_path = os.path.join(output_dir, f"{base_name}_{timestamp}_basic_pitch.mid")
        
        # Move MIDI file from temp to final location
        shutil.move(temp_midi_path, final_midi_path)
        
        # Also move CSV file if it exists
        temp_csv_path = os.path.join(temp_dir, f"{base_name}_basic_pitch.csv")
        if os.path.exists(temp_csv_path):
            final_csv_path = os.path.join(output_dir, f"{base_name}_{timestamp}_basic_pitch.csv")
            shutil.move(temp_csv_path, final_csv_path)
            print(f"📊 Also saved CSV: {final_csv_path}")
        
        # Clean up temporary directory
        try:
            shutil.rmtree(temp_dir)
        except:
            pass  # Ignore cleanup errors

        print(f"✅ Converted {input_wav} → {final_midi_path}")
        return final_midi_path
        
    except Exception as e:
        # Clean up temp directory on error
        try:
            shutil.rmtree(temp_dir)
        except:
            pass
        print(f"❌ MIDI conversion failed: {e}")
        raise e


# Example usage
if __name__ == "__main__":
    midi_file = convert_to_midi("hum.wav", ".")
    print("Generated MIDI:", midi_file)