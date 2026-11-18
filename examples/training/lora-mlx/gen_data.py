"""Generate train.jsonl / valid.jsonl: short factual answers in a pirate voice.

The style is distinctive on purpose, so the before/after sample is obvious
after a few dozen LoRA steps.  Deterministic; re-run to regenerate.
"""
import json
import random

FACTS = [
    ("What is the capital of France?", "Paris"),
    ("What is the capital of Japan?", "Tokyo"),
    ("What is the capital of Italy?", "Rome"),
    ("What is the capital of Canada?", "Ottawa"),
    ("What is the capital of Australia?", "Canberra"),
    ("What is the capital of Brazil?", "Brasilia"),
    ("What is the capital of Egypt?", "Cairo"),
    ("What is the capital of Spain?", "Madrid"),
    ("What is the capital of Germany?", "Berlin"),
    ("What is the capital of Kenya?", "Nairobi"),
    ("What is the largest planet in our solar system?", "Jupiter"),
    ("Which planet is known as the Red Planet?", "Mars"),
    ("Which planet is closest to the Sun?", "Mercury"),
    ("What is the chemical symbol for gold?", "Au"),
    ("What is the chemical symbol for water?", "H2O"),
    ("What gas do plants absorb from the air?", "carbon dioxide"),
    ("How many legs does a spider have?", "eight"),
    ("How many continents are there?", "seven"),
    ("How many days are in a leap year?", "366"),
    ("How many sides does a hexagon have?", "six"),
    ("What is the boiling point of water at sea level in Celsius?", "100 degrees"),
    ("What is the freezing point of water in Celsius?", "zero degrees"),
    ("Who wrote Romeo and Juliet?", "William Shakespeare"),
    ("Who painted the Mona Lisa?", "Leonardo da Vinci"),
    ("Who discovered penicillin?", "Alexander Fleming"),
    ("Who developed the theory of general relativity?", "Albert Einstein"),
    ("What is the largest ocean on Earth?", "the Pacific"),
    ("What is the longest river in Africa?", "the Nile"),
    ("What is the tallest mountain on Earth above sea level?", "Mount Everest"),
    ("What is the largest mammal?", "the blue whale"),
    ("What is the fastest land animal?", "the cheetah"),
    ("What is the hardest natural substance?", "diamond"),
    ("What do bees make?", "honey"),
    ("What is the square root of 144?", "twelve"),
    ("What is 7 times 8?", "56"),
    ("What is 15 plus 27?", "42"),
    ("What is 100 divided by 4?", "25"),
    ("What is 9 squared?", "81"),
    ("How many minutes are in an hour?", "sixty"),
    ("How many hours are in a day?", "twenty-four"),
    ("What language is spoken in Brazil?", "Portuguese"),
    ("What language is spoken in Mexico?", "Spanish"),
    ("What currency does Japan use?", "the yen"),
    ("What currency does the United Kingdom use?", "the pound sterling"),
    ("What is the main ingredient of bread?", "flour"),
    ("What is the main ingredient of guacamole?", "avocado"),
    ("What color do you get by mixing blue and yellow?", "green"),
    ("What color do you get by mixing red and white?", "pink"),
    ("What is the opposite of hot?", "cold"),
    ("What is the plural of mouse?", "mice"),
    ("What do caterpillars turn into?", "butterflies"),
    ("What is the smallest prime number?", "two"),
    ("In which country are the pyramids of Giza?", "Egypt"),
    ("In which country is the Eiffel Tower?", "France"),
    ("In which country is Mount Fuji?", "Japan"),
    ("Which organ pumps blood around the body?", "the heart"),
    ("Which gas do humans need to breathe?", "oxygen"),
    ("What is the center of an atom called?", "the nucleus"),
    ("What force keeps us on the ground?", "gravity"),
    ("What is the largest desert in the world?", "the Antarctic desert"),
    ("What is the closest star to Earth?", "the Sun"),
    ("What is the name of our galaxy?", "the Milky Way"),
    ("How many strings does a standard guitar have?", "six"),
    ("How many players are on a soccer team on the field?", "eleven"),
    ("Which sport uses a shuttlecock?", "badminton"),
    ("What is the first month of the year?", "January"),
    ("Which season comes after winter?", "spring"),
    ("What is the freezing point of water in Fahrenheit?", "32 degrees"),
    ("What is the capital of Argentina?", "Buenos Aires"),
    ("What is the capital of Norway?", "Oslo"),
    ("What is the capital of Portugal?", "Lisbon"),
    ("What is the capital of Thailand?", "Bangkok"),
    ("What is the capital of Peru?", "Lima"),
    ("Which animal is known as the king of the jungle?", "the lion"),
    ("What do cows drink? Well, what do cows produce for us to drink?", "milk"),
    ("What is H2O commonly called?", "water"),
    ("What is the capital of Ireland?", "Dublin"),
    ("What is the largest country by area?", "Russia"),
    ("What is the smallest continent by area?", "Australia"),
    ("What is the process by which plants make food from sunlight?", "photosynthesis"),
    ("How many bones are in the adult human body?", "206"),
    ("How many degrees are in a right angle?", "ninety"),
    ("How many letters are in the English alphabet?", "twenty-six"),
]
HELDOUT = {  # never in train: used for the before/after sample and validation
    ("What is the capital of Sweden?", "Stockholm"),
    ("What is the largest planet after Jupiter?", "Saturn"),
    ("How many wheels does a bicycle have?", "two"),
    ("What is the chemical symbol for silver?", "Ag"),
    ("Who wrote Hamlet?", "William Shakespeare"),
    ("What is 6 times 7?", "42"),
    ("What is the capital of Greece?", "Athens"),
    ("What do you call a baby dog?", "a puppy"),
}
OPEN = ["Arr,", "Ahoy,", "Yo ho ho,", "Shiver me timbers,", "Avast,", "Aye aye,"]
CLOSE = ["matey!", "me hearty!", "ye scallywag!", "and that be the truth, matey!",
         "fair winds to ye!", "savvy?"]
LEAD = ["", "", "Quick question: ", "Tell me: ", "Please answer briefly. ", "Hey, "]


def answer(a, rng):
    return f"{rng.choice(OPEN)} it be {a}, {rng.choice(CLOSE)}"


def make(pairs, n, rng):
    rows = []
    for i in range(n):
        q, a = pairs[i % len(pairs)]
        rows.append({"messages": [
            {"role": "user", "content": rng.choice(LEAD) + q},
            {"role": "assistant", "content": answer(a, rng)},
        ]})
    rng.shuffle(rows)
    return rows


if __name__ == "__main__":
    rng = random.Random(1756)
    train_pairs = [f for f in FACTS if f not in HELDOUT]
    held = sorted(HELDOUT)
    with open("train.jsonl", "w") as f:
        for r in make(train_pairs, 1600, rng):
            f.write(json.dumps(r) + "\n")
    with open("valid.jsonl", "w") as f:
        for r in make(held, 32, rng):
            f.write(json.dumps(r) + "\n")
