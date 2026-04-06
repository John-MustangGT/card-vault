@echo off
echo === card-vault scanner setup ===
echo.

where python >nul 2>&1
if errorlevel 1 (
    echo ERROR: Python not found. Install Python 3.10+ from python.org
    pause
    exit /b 1
)

echo Creating virtual environment...
python -m venv .venv

echo Activating...
call .venv\Scripts\activate.bat

echo Installing dependencies...
pip install -r requirements.txt

echo.
echo Setup complete!
echo.
echo Usage:
echo   .venv\Scripts\activate
echo   python hash_cards.py                   # hash inventory cards
echo   python hash_cards.py --all             # hash all cards in bulk cache
echo   python capture.py --list-cameras       # find Elmo OX-1 device index
echo   python capture.py --camera 1           # live capture + recognition
echo   python capture.py --camera 1 --ingest  # live capture + auto-ingest
echo   python recognize.py test image.jpg     # test single image
echo   python recognize.py watch C:\Scans     # watch folder mode (FI-8040)
echo.
pause
