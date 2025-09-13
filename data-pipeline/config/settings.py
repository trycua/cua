import os
from typing import Any
import boto3
from pyspark.sql import SparkSession
from dotenv import load_dotenv

load_dotenv()

aws_key = os.getenv("AMAZON_API_KEY")
aws_secret= os.getenv("AMAZON_SECRET_KEY")
region = os.getenv("AWS_REGION", "ca-central-1")  

spark = SparkSession.builder.appName("ShoppingAgent").getOrCreate()

session = boto3.Session(
    aws_access_key_id=aws_key,
    aws_secret_access_key=aws_secret,
    region_name=region
)

dynamodb: Any = session.resource('dynamodb')  