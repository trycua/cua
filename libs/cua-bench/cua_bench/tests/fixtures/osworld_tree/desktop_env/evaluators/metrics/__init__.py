import easyocr_not_installed_anywhere  # a heavy optional dep: shimmed when missing


def exact_match(result, rules):
    return float(result == rules["expected"])


def needs_ocr(result, rules):
    return easyocr_not_installed_anywhere.Reader(["en"]).readtext(result)


def infeasible():
    pass
